use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib::object::IsA;
use gtk4::prelude::*;
use gtk4::{glib, pango};
use text_workspace::markdown;
use text_workspace::markup::{mark_spans, MarkKind, MarkSpan};
use text_workspace::session::ViewMode;
use text_workspace::tables::{self, ColAlign, MdTable};
use text_workspace::zoom;

use super::support::{buffer_text_of, is_markdown, menu_row, scrolled_of};
use super::{Ctx, State, PREVIEW_MS};

fn display_line_number_of(state: &State, buffer_line: i32) -> Option<i32> {
    if buffer_line < 0 {
        return None;
    }
    if state.view_mode != ViewMode::Edit || state.render.tables.is_empty() {
        return Some(buffer_line + 1);
    }
    let mut hidden = 0i32;
    for table in &state.render.tables {
        if buffer_line > table.start_line && buffer_line < table.end_line {
            return None;
        }
        if buffer_line >= table.end_line {
            hidden += (table.end_line - table.start_line - 1).max(0);
        }
    }
    Some(buffer_line + 1 - hidden)
}

const TAG_HEADING: &[&str] = &["tw-h1", "tw-h2", "tw-h3", "tw-h4", "tw-h5", "tw-h6"];
const TAG_STRONG: &str = "tw-strong";
const TAG_EM: &str = "tw-em";
const TAG_CODE: &str = "tw-code";
const TAG_CODEBLOCK: &str = "tw-codeblock";
const TAG_HIDE: &str = "tw-hide";
const TAG_TABLE: &str = "tw-table";
const TAG_ZOOM: &str = "tw-zoom";
const TAG_LINK: &str = "tw-link";
const TAG_MONO: &str = "tw-mono";

#[derive(Default)]
pub(super) struct RenderState {
    pub writing: bool,
    tick: u64,
    tables: Vec<LiveTable>,
    images: Vec<LiveImage>,
    /// Разбор текущего текста. Курсор его не пересчитывает.
    spans: Vec<MarkSpan>,
    line_starts: Vec<usize>,
    text_len: usize,
    /// По строке: хеш текста и хеш её разметки. По ним быстрая перерисовка
    /// находит строки, чьё оформление изменилось, кроме строки курсора.
    line_sigs: Vec<(u64, u64)>,
    cursor_line: Option<i32>,
    painted: bool,
    /// Файл, для которого посчитаны spans, таблицы и картинки.
    /// Буфер другого файла этими данными трогать нельзя: якоря чужие.
    painted_path: Option<PathBuf>,
    table_bodies: Vec<String>,
    image_keys: Vec<String>,
    pending_table_focus: Option<(usize, usize, usize)>,
    parking: bool,
    cell_caret: i32,
    cell_anchor: i32,
    cell_typing: bool,
    cell_undo: Vec<(String, i32, i32)>,
    cell_redo: Vec<(String, i32, i32)>,
    /// Пока буфер откатывается, курсор не перекрашивает строки старыми смещениями.
    cursor_suspend: bool,
    /// Рендер отложен, пока текстовое поле ещё не показано.
    wait_realize: bool,
    /// Высота курсора ячейки для размера текста: (text_pt, пиксели).
    caret_px: Option<(i32, i32)>,
}

#[derive(Clone)]
struct LiveTable {
    path: PathBuf,
    index: usize,
    start: usize,
    end: usize,
    start_line: i32,
    end_line: i32,
    anchor: gtk4::TextChildAnchor,
    /// Сетка для каждой панели. Вторая строится, только когда окно разделено.
    grids: [Option<gtk4::Grid>; 2],
    focus_cell: Rc<RefCell<Option<(usize, usize)>>>,
}

#[derive(Clone)]
struct LiveImage {
    path: PathBuf,
    anchor: gtk4::TextChildAnchor,
    widgets: [Option<gtk4::Widget>; 2],
}

impl Ctx {
    /// Рендер посчитан для этого файла.
    fn render_owns(&self, path: &Path) -> bool {
        let state = self.state.borrow();
        state.render.painted && state.render.painted_path.as_deref() == Some(path)
    }

    /// Рендер посчитан для файла этого буфера, и этот файл сейчас в фокусе.
    fn render_owns_buffer(&self, buffer: &sourceview5::Buffer) -> bool {
        let Some(focused) = self.focused_path() else {
            return false;
        };
        let state = self.state.borrow();
        if !state.render.painted || state.render.painted_path.as_deref() != Some(focused.as_path())
        {
            return false;
        }
        state
            .buffers
            .get(&focused)
            .is_some_and(|entry| &entry.buffer == buffer)
    }

    pub(super) fn install_render_clicks(&self) {
        for view in &self.widgets.views {
            // Один обработчик на вид. Раньше каждое ожидание добавляло новый.
            let ctx = self.clone();
            view.connect_realize(move |_| {
                if !ctx.state.borrow().render.wait_realize {
                    return;
                }
                ctx.state.borrow_mut().render.wait_realize = false;
                let ctx = ctx.clone();
                glib::idle_add_local_once(move || ctx.refresh_render_now());
            });
            let click = gtk4::GestureClick::new();
            click.set_button(1);
            let ctx = self.clone();
            let view_click = view.clone();
            click.connect_pressed(move |gesture, _, x, y| {
                if !ctx.editing_markdown() {
                    return;
                }
                if !pick_is_table_cell(&view_click, x, y) {
                    ctx.deactivate_table_cells();
                }
                let Some(event) = gesture.current_event() else {
                    return;
                };
                if !event
                    .modifier_state()
                    .contains(gdk::ModifierType::CONTROL_MASK)
                {
                    return;
                }
                let Ok(buffer) = view_click.buffer().downcast::<sourceview5::Buffer>() else {
                    return;
                };
                let Some(iter) = view_click.iter_at_location(x as i32, y as i32) else {
                    return;
                };
                let offset = file_byte_at_iter(&buffer.clone().upcast(), &iter);
                let Some(path) = ctx.focused_path() else {
                    return;
                };
                let spans = ctx.state.borrow().render.spans.clone();
                for span in &spans {
                    if offset < span.start || offset >= span.end {
                        continue;
                    }
                    match &span.kind {
                        MarkKind::Link { dest } => {
                            if let Some(target) = markdown::resolve_markdown(&path, dest) {
                                ctx.go(target, None);
                            }
                        }
                        MarkKind::WikiLink { target } => ctx.pick_wiki(target),
                        _ => continue,
                    }
                    gesture.set_state(gtk4::EventSequenceState::Claimed);
                    return;
                }
            });
            view.add_controller(click);
            let keys = gtk4::EventControllerKey::new();
            keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
            let ctx = self.clone();
            keys.connect_key_pressed(move |_, keyval, _, modifier| {
                if !ctx.editing_markdown() {
                    return glib::Propagation::Proceed;
                }
                ctx.handle_table_key(keyval, modifier)
            });
            view.add_controller(keys);
            let bubble = gtk4::EventControllerKey::new();
            bubble.set_propagation_phase(gtk4::PropagationPhase::Bubble);
            let ctx = self.clone();
            bubble.connect_key_pressed(move |_, _, _, _| {
                if ctx.any_table_cell_focused() {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            view.add_controller(bubble);
        }
    }

    /// Markdown в редактировании показывает собранный текст и таблицы.
    pub(super) fn editing_markdown(&self) -> bool {
        if self.state.borrow().view_mode != ViewMode::Edit {
            return false;
        }
        self.focused_path().is_some_and(|path| is_markdown(&path))
    }

    pub(super) fn schedule_render(&self) {
        if !self.editing_markdown() {
            return;
        }
        let tick = {
            let mut state = self.state.borrow_mut();
            state.render.tick = state.render.tick.wrapping_add(1);
            state.render.tick
        };
        let ctx = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(PREVIEW_MS), move || {
            if ctx.state.borrow().render.tick != tick {
                return;
            }
            if ctx.state.borrow().render.writing {
                return;
            }
            if !ctx.editing_markdown() {
                return;
            }
            ctx.refresh_render_now();
        });
    }

    pub(super) fn refresh_render_now(&self) {
        let _prof = crate::ui::prof::span("refresh_render_now");
        if !self.editing_markdown() {
            self.clear_render_layer();
            return;
        }
        let Some(path) = self.focused_path() else {
            self.clear_render_layer();
            return;
        };
        if !is_markdown(&path) {
            self.clear_render_layer();
            return;
        }
        if self.state.borrow().render.writing {
            return;
        }
        if !self.focused_view().is_realized() {
            self.render_after_realize();
            return;
        }
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        self.ensure_render_tags(&buffer);
        self.ensure_cursor_hook(&path, &buffer);
        let cursor_line = buffer.iter_at_mark(&buffer.get_insert()).line();
        if self.any_table_cell_focused() {
            if self.render_owns(&path) {
                self.recover_table_cover(&buffer);
            }
            return;
        }
        self.rebuild_widgets(&buffer, &path, cursor_line);
        self.restore_pending_view_line();
    }

    /// Якоря таблиц в ещё не показанном тексте роняют GTK.
    fn render_after_realize(&self) {
        if self.state.borrow().render.wait_realize {
            return;
        }
        self.state.borrow_mut().render.wait_realize = true;
        let view = self.focused_view();
        if view.is_realized() {
            self.state.borrow_mut().render.wait_realize = false;
            let ctx = self.clone();
            glib::idle_add_local_once(move || ctx.refresh_render_now());
        }
    }

    fn restore_pending_view_line(&self) {
        let Some(line) = self.state.borrow().pending_view_line else {
            return;
        };
        if self.scroll_editor_to_line(line) {
            self.state.borrow_mut().pending_view_line = None;
        }
    }

    /// Для замеров: сколько сеток таблиц в первой панели.
    pub(super) fn first_pane_grid_count(&self) -> usize {
        self.state
            .borrow()
            .render
            .tables
            .iter()
            .filter(|t| t.grids[0].is_some())
            .count()
    }

    /// Для замеров: сколько таблиц сейчас показано виджетами (якорь жив).
    pub(super) fn live_table_count(&self) -> usize {
        self.state
            .borrow()
            .render
            .tables
            .iter()
            .filter(|t| !t.anchor.is_deleted())
            .count()
    }

    /// Для замеров: какие теги стоят на первых служебных `##` вне строки курсора.
    pub(super) fn markup_report(&self) -> String {
        let Some(path) = self.focused_path() else {
            return "нет файла".into();
        };
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|e| e.buffer.clone())
        else {
            return "нет буфера".into();
        };
        let cursor = buffer.iter_at_mark(&buffer.get_insert()).line();
        let mut out = Vec::new();
        for line in 0..buffer.line_count() {
            if out.len() >= 2 || line == cursor {
                continue;
            }
            let Some(iter) = buffer.iter_at_line(line) else {
                continue;
            };
            let mut end = iter;
            end.forward_chars(3);
            if !buffer.text(&iter, &end, true).starts_with("##") {
                continue;
            }
            let tags: Vec<String> = iter
                .tags()
                .iter()
                .map(|t| {
                    let name = t
                        .name()
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "?".into());
                    let fg = if t.is_foreground_set() {
                        t.foreground_rgba()
                            .map(|c| {
                                format!(
                                    "{:.2},{:.2},{:.2},{:.2}",
                                    c.red(),
                                    c.green(),
                                    c.blue(),
                                    c.alpha()
                                )
                            })
                            .unwrap_or_default()
                    } else {
                        "-".into()
                    };
                    format!("{name}[p{} fg {fg}]", t.priority())
                })
                .collect();
            out.push(format!("line {line}: {}", tags.join(" ")));
        }
        format!(
            "tag_table {} | {}",
            buffer.tag_table().size(),
            out.join(" || ")
        )
    }

    /// Высота курсора ячейки. Считается один раз на размер текста.
    pub(super) fn cell_caret_height(&self, entry: &gtk4::Entry) -> i32 {
        let pt = self.state.borrow().session.text_pt;
        if let Some((cached_pt, px)) = self.state.borrow().render.caret_px {
            if cached_pt == pt {
                return px;
            }
        }
        let px = measure_line_height(entry, pt);
        self.state.borrow_mut().render.caret_px = Some((pt, px));
        px
    }

    /// Масштаб текста изменился: новая высота у уже созданных курсоров.
    pub(super) fn resize_cell_carets(&self) {
        let grids: Vec<gtk4::Grid> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .flat_map(|table| table.grids.iter().flatten().cloned())
            .collect();
        for grid in grids {
            let mut child = grid.first_child();
            while let Some(cell) = child {
                child = cell.next_sibling();
                let Some(entry) = cell_entry_of(&cell) else {
                    continue;
                };
                let Some(bar) = cell
                    .clone()
                    .downcast::<gtk4::Overlay>()
                    .ok()
                    .and_then(|overlay| overlay_caret_bar(&overlay))
                    .and_then(|bar| bar.downcast::<gtk4::DrawingArea>().ok())
                else {
                    continue;
                };
                let px = self.cell_caret_height(&entry);
                if bar.content_height() != px {
                    bar.set_content_height(px);
                }
            }
        }
    }

    /// Для замеров: сделать ячейку активной и вернуть высоту курсора, высоту
    /// строки шрифта поля и высоту ячейки.
    pub(super) fn bench_caret_in_cell(&self, table: usize, row: usize, col: usize) {
        self.activate_table_cell(table, row, col);
    }

    pub(super) fn bench_caret_report(&self) -> String {
        let Some((table, row, col)) = self.focused_table_cell() else {
            return "нет активной ячейки".into();
        };
        let Some(entry) = self.table_entry(table, row, col) else {
            return "нет поля".into();
        };
        let Some(overlay) = entry
            .parent()
            .and_then(|p| p.downcast::<gtk4::Overlay>().ok())
        else {
            return "нет ячейки".into();
        };
        let bar = overlay_caret_bar(&overlay)
            .map(|b| b.height())
            .unwrap_or(-1);
        format!(
            "ячейка ({row},{col}): курсор {bar} px, строка поля {} px, ячейка {} px",
            entry.create_pango_layout(Some("Ag")).pixel_size().1,
            overlay.height()
        )
    }

    /// Для замеров: вписать текст в активную ячейку таблицы.
    pub(super) fn bench_type_in_cell(&self, text: &str) -> bool {
        let Some((table, row, col)) = self.focused_table_cell() else {
            return false;
        };
        let Some(entry) = self.table_entry(table, row, col) else {
            return false;
        };
        entry.set_text(text);
        true
    }

    /// Для замеров: таблиц, сеток второй панели, из них показанных.
    pub(super) fn table_grid_report(&self) -> (usize, usize, usize) {
        // (таблиц, сеток во второй панели, из них показанных)
        let state = self.state.borrow();
        let tables = &state.render.tables;
        let second: Vec<_> = tables.iter().filter_map(|t| t.grids[1].clone()).collect();
        let mapped = second.iter().filter(|g| g.is_mapped()).count();
        (tables.len(), second.len(), mapped)
    }

    /// Пересобрать таблицы и картинки с нуля, например после смены разделения окна.
    pub(super) fn rebuild_render(&self) {
        self.clear_render_layer();
        self.refresh_render_now();
    }

    pub(super) fn clear_render_layer(&self) {
        let (tables, images) = {
            let mut state = self.state.borrow_mut();
            state.render.spans.clear();
            state.render.line_starts.clear();
            state.render.line_sigs.clear();
            state.render.table_bodies.clear();
            state.render.image_keys.clear();
            state.render.painted = false;
            state.render.painted_path = None;
            state.render.cursor_line = None;
            (
                std::mem::take(&mut state.render.tables),
                std::mem::take(&mut state.render.images),
            )
        };
        self.drop_embeds(&tables, &images);
    }

    pub(super) fn drop_render_for_path(&self, path: &Path) {
        if self.focused_table_cell().is_some() {
            self.deactivate_table_cells();
        }
        let (tables, images) = {
            let mut state = self.state.borrow_mut();
            let tables = std::mem::take(&mut state.render.tables);
            let images = std::mem::take(&mut state.render.images);
            let keep_tables: Vec<_> = tables
                .iter()
                .filter(|table| table.path != path)
                .cloned()
                .collect();
            let keep_images: Vec<_> = images
                .iter()
                .filter(|image| image.path != path)
                .cloned()
                .collect();
            let drop_tables: Vec<_> = tables
                .into_iter()
                .filter(|table| table.path == path)
                .collect();
            let drop_images: Vec<_> = images
                .into_iter()
                .filter(|image| image.path == path)
                .collect();
            if keep_tables.is_empty() && keep_images.is_empty() {
                state.render.spans.clear();
                state.render.line_starts.clear();
                state.render.line_sigs.clear();
                state.render.table_bodies.clear();
                state.render.image_keys.clear();
                state.render.painted = false;
                state.render.painted_path = None;
                state.render.cursor_line = None;
            }
            if state.render.painted_path.as_deref() == Some(path) {
                state.render.painted = false;
                state.render.painted_path = None;
                state.render.cursor_line = None;
            }
            state.render.tables = keep_tables;
            state.render.images = keep_images;
            (drop_tables, drop_images)
        };
        self.drop_embeds(&tables, &images);
    }

    fn drop_embeds(&self, tables: &[LiveTable], images: &[LiveImage]) {
        let _prof = crate::ui::prof::span("drop_embeds");
        for table in tables {
            for grid in table.grids.iter().flatten() {
                detach_child(grid);
            }
        }
        for image in images {
            for widget in image.widgets.iter().flatten() {
                detach_child(widget);
            }
        }
        let mut paths: Vec<PathBuf> = tables.iter().map(|table| table.path.clone()).collect();
        paths.extend(images.iter().map(|image| image.path.clone()));
        paths.sort();
        paths.dedup();
        for path in paths {
            let Some(buffer) = self
                .state
                .borrow()
                .buffers
                .get(&path)
                .map(|entry| entry.buffer.clone())
            else {
                continue;
            };
            self.state.borrow_mut().render.writing = true;
            remove_path_anchors(
                &buffer.clone().upcast::<gtk4::TextBuffer>(),
                tables,
                images,
                &path,
            );
            self.state.borrow_mut().render.writing = false;
            clear_render_tags(&buffer);
        }
    }

    pub(super) fn ensure_line_gutter(&self, view: &sourceview5::View) {
        let pane = if std::ptr::eq(view, &self.widgets.views[0]) {
            0
        } else {
            1
        };
        if self.state.borrow().gutter_installed[pane] {
            return;
        }
        let gutter = sourceview5::prelude::ViewExt::gutter(view, gtk4::TextWindowType::Left);
        let renderer = sourceview5::GutterRendererText::new();
        gtk4::prelude::WidgetExt::set_widget_name(&renderer, "tw-line-numbers");
        gtk4::prelude::WidgetExt::set_width_request(&renderer, 48);
        sourceview5::prelude::GutterRendererExt::set_xalign(&renderer, 1.0);
        sourceview5::prelude::GutterRendererExt::set_xpad(&renderer, 8);
        sourceview5::prelude::GutterRendererTextExt::set_text(&renderer, "8888");
        let ctx = self.clone();
        sourceview5::prelude::GutterRendererExt::connect_query_data(
            &renderer,
            move |renderer, _, line| {
                let text = match ctx.state.try_borrow() {
                    Ok(state) => display_line_number_of(&state, line as i32)
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    Err(_) => String::new(),
                };
                sourceview5::prelude::GutterRendererTextExt::set_text(renderer, &text);
            },
        );
        sourceview5::prelude::ViewExt::set_show_line_numbers(view, false);
        if gutter.insert(&renderer, 0) {
            self.state.borrow_mut().gutter_installed[pane] = true;
        }
    }

    pub(super) fn watch_table_widths(&self) {
        for pane in 0..2 {
            let ctx = self.clone();
            self.widgets.views[pane].connect_realize(move |view| {
                ctx.sync_table_grid_widths();
                if let Some(scroll) = scrolled_of(view) {
                    let ctx = ctx.clone();
                    scroll
                        .hadjustment()
                        .connect_notify_local(Some("page-size"), move |_, _| {
                            ctx.sync_table_grid_widths();
                        });
                }
            });
        }
        let ctx = self.clone();
        self.widgets
            .editor_paned
            .connect_notify_local(Some("position"), move |_, _| {
                ctx.sync_table_grid_widths();
            });
        let ctx = self.clone();
        self.widgets
            .main_paned
            .connect_notify_local(Some("position"), move |_, _| {
                ctx.sync_table_grid_widths();
            });
        let ctx = self.clone();
        self.widgets
            .right_paned
            .connect_notify_local(Some("position"), move |_, _| {
                ctx.sync_table_grid_widths();
            });
    }

    pub(super) fn sync_table_grid_widths(&self) {
        let _prof = crate::ui::prof::span("sync_table_grid_widths");
        let tables: Vec<[Option<gtk4::Grid>; 2]> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| table.grids.clone())
            .collect();
        for grids in tables {
            for (pane, grid) in grids.iter().enumerate() {
                let Some(grid) = grid else {
                    continue;
                };
                size_table_grid(grid, &self.widgets.views[pane]);
            }
        }
    }

    pub(super) fn apply_text_zoom_tags(&self) {
        let _prof = crate::ui::prof::span("apply_text_zoom_tags");
        let pt = self.state.borrow().session.text_pt;
        let scale = zoom::webkit_factor(pt);
        let buffers: Vec<_> = self
            .state
            .borrow()
            .buffers
            .values()
            .map(|entry| entry.buffer.clone())
            .collect();
        for buffer in buffers
            .iter()
            .chain(std::iter::once(&self.widgets.empty_buffer))
        {
            ensure_tag(&buffer.tag_table(), TAG_ZOOM, |tag| {
                tag.set_scale(scale);
            });
            let start = buffer.start_iter();
            let end = buffer.end_iter();
            if let Some(tag) = buffer.tag_table().lookup(TAG_ZOOM) {
                buffer.apply_tag(&tag, &start, &end);
            }
        }
        if !self.editing_markdown() {
            return;
        }
        let Some(path) = self.focused_path() else {
            return;
        };
        if !self.render_owns(&path) {
            return;
        }
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        let text = buffer_text_of(&buffer);
        let tables: Vec<(usize, usize)> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| (table.start, table.end))
            .collect();
        let table_anchors: Vec<_> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| table.anchor.clone())
            .collect();
        let image_anchors: Vec<_> = self
            .state
            .borrow()
            .render
            .images
            .iter()
            .map(|image| image.anchor.clone())
            .collect();
        let map = TextMap::new(&buffer, &text);
        for (start, end) in tables {
            cover_table_source(&buffer, &text, &map, start, end);
        }
        for anchor in &table_anchors {
            lift_anchor_scale(&buffer, anchor);
        }
        for anchor in &image_anchors {
            lift_anchor_scale(&buffer, anchor);
        }
    }

    pub(super) fn apply_render_mode_chrome(&self) {
        let mode = self.state.borrow().view_mode;
        let path = self.focused_path();
        let markdown = path.as_ref().is_some_and(|path| is_markdown(path));
        let rendered = mode == ViewMode::Edit && markdown;
        for view in &self.widgets.views {
            view.set_monospace(!rendered);
        }
        if rendered {
            self.schedule_render();
        } else {
            self.deactivate_table_cells();
            self.clear_render_layer();
        }
    }

    pub(super) fn add_table_column(&self) {
        let Some((path, table, row, col)) = self.table_edit_target() else {
            self.set_status("Нет таблицы под курсором");
            return;
        };
        self.table_insert_column(path, table, row, col, true);
    }

    pub(super) fn commit_table_cell(
        &self,
        path: PathBuf,
        table: usize,
        row: usize,
        col: usize,
        value: String,
    ) {
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        let text = buffer_text_of(&buffer);
        let Some(cell) = tables::parse_tables(&text)
            .get(table)
            .and_then(|item| item.cell(row, col))
            .cloned()
        else {
            return;
        };
        if cell.text == value {
            return;
        }
        self.state.borrow_mut().render.writing = true;
        let mut start = iter_at_file_byte(&buffer, &text, cell.start);
        let mut end = iter_at_file_byte(&buffer, &text, cell.end);
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &value);
        buffer.end_user_action();
        self.state.borrow_mut().render.writing = false;
        self.note_text_edit(&path, false);
        // Синхронизировать текст ячеек во втором поле.
        self.sync_table_cell_entries(table, row, col, value.trim());
    }

    pub(super) fn table_add_row(&self, path: PathBuf, table: usize) {
        let row = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .find(|item| item.index == table)
            .and_then(|item| item.focus_cell.borrow().map(|(row, _)| row))
            .unwrap_or(0);
        self.table_insert_row(path, table, row, 0, true);
    }

    fn table_edit_target(&self) -> Option<(PathBuf, usize, usize, usize)> {
        if !self.editing_markdown() {
            self.set_status("Таблицу правят при редактировании Markdown");
            return None;
        }
        let path = self.focused_path()?;
        if !is_markdown(&path) {
            return None;
        }
        let (table, row, col) = self.focused_table_cell().or_else(|| {
            self.state
                .borrow()
                .render
                .tables
                .first()
                .map(|item| (item.index, 0, 0))
        })?;
        Some((path, table, row, col))
    }

    /// Совпадение поиска внутри исходника таблицы: открыть ячейку с ним и
    /// запомнить конец таблицы, чтобы следующий поиск шёл дальше.
    /// `false` — совпадение не в таблице.
    pub(super) fn open_find_hit_in_table(
        &self,
        buffer: &gtk4::TextBuffer,
        hit: &gtk4::TextIter,
    ) -> bool {
        let Some(source) = buffer.downcast_ref::<sourceview5::Buffer>() else {
            return false;
        };
        let text = buffer_text_of(source);
        let byte = file_byte_at_iter(buffer, hit);
        let tables = tables::parse_tables(&text);
        let Some((index, table)) = tables
            .iter()
            .enumerate()
            .find(|(_, table)| table.start <= byte && byte < table.end)
        else {
            return false;
        };
        if !self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .any(|live| live.index == index && !live.anchor.is_deleted())
        {
            return false;
        }
        let mut cell_at = None;
        for row in 0..table.row_count() {
            for col in 0..table.column_count() {
                if let Some(cell) = table.cell(row, col) {
                    if cell.start <= byte && byte <= cell.end {
                        cell_at = Some((row, col));
                    }
                }
            }
        }
        let (row, col) = cell_at.unwrap_or((0, 0));
        let end = TextMap::new(source, &text).iter(source, &text, table.end);
        match buffer.mark("tw-find-resume") {
            Some(mark) => buffer.move_mark(&mark, &end),
            None => {
                buffer.create_mark(Some("tw-find-resume"), &end, true);
            }
        }
        self.activate_table_cell(index, row, col);
        true
    }

    pub(super) fn focused_table_cell(&self) -> Option<(usize, usize, usize)> {
        let state = self.state.borrow();
        state.render.tables.iter().find_map(|item| {
            item.focus_cell
                .borrow()
                .map(|(row, col)| (item.index, row, col))
        })
    }

    fn table_insert_row(&self, path: PathBuf, table: usize, row: usize, col: usize, below: bool) {
        let at = if below { row } else { row.saturating_sub(1) };
        let focus_row = if below { row + 1 } else { row.max(1) };
        self.apply_table_edit(&path, (table, focus_row, col), |text| {
            tables::insert_row_at(text, table, at)
        });
    }

    fn table_delete_row(&self, path: PathBuf, table: usize, row: usize, col: usize) {
        if row == 0 {
            self.set_status("Заголовок таблицы не удаляется");
            return;
        }
        self.apply_table_edit(&path, (table, row - 1, col), |text| {
            tables::delete_row_at(text, table, row)
        });
    }

    fn table_insert_column(
        &self,
        path: PathBuf,
        table: usize,
        row: usize,
        col: usize,
        right: bool,
    ) {
        let at = if right { col + 1 } else { col };
        let focus_col = at;
        self.apply_table_edit(&path, (table, row, focus_col), |text| {
            tables::insert_column_at(text, table, at)
        });
    }

    fn table_delete_column(&self, path: PathBuf, table: usize, row: usize, col: usize) {
        self.apply_table_edit(&path, (table, row, col.saturating_sub(1)), |text| {
            tables::delete_column_at(text, table, col)
        });
    }

    fn apply_table_edit(
        &self,
        path: &Path,
        focus: (usize, usize, usize),
        edit: impl FnOnce(&str) -> Option<String>,
    ) {
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        let text = buffer_text_of(&buffer);
        let Some(next) = edit(&text) else {
            self.set_status("Так таблицу изменить нельзя");
            return;
        };
        self.state.borrow_mut().render.pending_table_focus = Some(focus);
        self.replace_all_text(&buffer, &next);
        self.note_text_edit(path, false);
        self.refresh_render_now();
    }

    fn restore_table_focus(&self) {
        let _prof = crate::ui::prof::span("restore_table_focus");
        let Some((table, row, col)) = self.state.borrow_mut().render.pending_table_focus.take()
        else {
            return;
        };
        let ctx = self.clone();
        glib::idle_add_local_once(move || {
            ctx.activate_table_cell(table, row, col);
        });
    }

    fn show_table_menu(
        &self,
        anchor: &gtk4::Widget,
        path: PathBuf,
        table: usize,
        row: usize,
        col: usize,
    ) {
        let popover = gtk4::Popover::new();
        popover.add_css_class("context-menu");
        popover.set_parent(anchor);
        popover.set_has_arrow(true);
        let menu = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let add = |title: &str, on: Box<dyn Fn()>| {
            let row_btn = menu_row(title);
            let popover_close = popover.clone();
            row_btn.connect_clicked(move |_| {
                on();
                popover_close.popdown();
            });
            menu.append(&row_btn);
        };
        let ctx = self.clone();
        let path_c = path.clone();
        add(
            "Строка выше",
            Box::new(move || ctx.table_insert_row(path_c.clone(), table, row, col, false)),
        );
        let ctx = self.clone();
        let path_c = path.clone();
        add(
            "Строка ниже",
            Box::new(move || ctx.table_insert_row(path_c.clone(), table, row, col, true)),
        );
        let ctx = self.clone();
        let path_c = path.clone();
        add(
            "Удалить строку",
            Box::new(move || ctx.table_delete_row(path_c.clone(), table, row, col)),
        );
        let ctx = self.clone();
        let path_c = path.clone();
        add(
            "Столбец слева",
            Box::new(move || ctx.table_insert_column(path_c.clone(), table, row, col, false)),
        );
        let ctx = self.clone();
        let path_c = path.clone();
        add(
            "Столбец справа",
            Box::new(move || ctx.table_insert_column(path_c.clone(), table, row, col, true)),
        );
        let ctx = self.clone();
        let path_c = path;
        add(
            "Удалить столбец",
            Box::new(move || ctx.table_delete_column(path_c.clone(), table, row, col)),
        );
        popover.set_child(Some(&menu));
        popover.connect_closed(|popover| {
            popover.unparent();
        });
        popover.popup();
    }

    fn sync_table_cell_entries(&self, table: usize, row: usize, col: usize, value: &str) {
        let state = self.state.borrow();
        let Some(live) = state.render.tables.iter().find(|item| item.index == table) else {
            return;
        };
        for grid in live.grids.iter().flatten() {
            let Some(cell) = grid.child_at(col as i32, row as i32) else {
                continue;
            };
            if let Some(entry) = cell_entry_of(&cell) {
                if entry.text() != value {
                    entry.set_text(value);
                }
            } else if let Some(label) = cell_label_of(&cell) {
                if label.text() != value {
                    label.set_text(value);
                }
            }
        }
    }

    /// Заменить текст буфера на `next`, переписав только отличающийся кусок.
    /// `set_text` стирал всю историю отмены, и Ctrl+Z после вставки строки
    /// или колонки таблицы переставал работать.
    fn replace_all_text(&self, buffer: &sourceview5::Buffer, next: &str) {
        let text = buffer_text_of(buffer);
        let (from, old_to, new_to) = changed_span(&text, next);
        if from == old_to && from == new_to {
            return;
        }
        self.state.borrow_mut().render.writing = true;
        let map = TextMap::new(buffer, &text);
        let mut start = map.iter(buffer, &text, from);
        let mut end = map.iter(buffer, &text, old_to);
        buffer.begin_user_action();
        buffer.delete(&mut start, &mut end);
        buffer.insert(&mut start, &next[from..new_to]);
        buffer.end_user_action();
        self.state.borrow_mut().render.writing = false;
    }

    pub(super) fn any_table_cell_focused(&self) -> bool {
        self.focused_table_cell().is_some()
    }

    pub(super) fn suspend_cursor_paint(&self, suspend: bool) {
        self.state.borrow_mut().render.cursor_suspend = suspend;
    }

    fn lock_source_input(&self, lock: bool) {
        let allow = self.state.borrow().view_mode == ViewMode::Edit && !lock;
        for view in &self.widgets.views {
            view.set_editable(allow);
            view.set_cursor_visible(allow);
            view.set_can_focus(allow);
            view.action_set_enabled("text.undo", !lock);
            view.action_set_enabled("text.redo", !lock);
            gtk4::prelude::TextViewExt::reset_im_context(view);
        }
    }

    fn activate_table_cell(&self, table: usize, row: usize, col: usize) {
        self.table_entry(table, row, col);
        if self.focused_table_cell() != Some((table, row, col)) {
            if let Some((prev_t, prev_r, prev_c)) = self.focused_table_cell() {
                self.commit_active_cell(prev_t, prev_r, prev_c);
            }
            for item in &self.state.borrow().render.tables {
                *item.focus_cell.borrow_mut() = if item.index == table {
                    Some((row, col))
                } else {
                    None
                };
            }
        }
        self.paint_cell_active_class();
        self.lock_source_input(true);
        let Some(path) = self.focused_path() else {
            return;
        };
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        self.state.borrow_mut().render.parking = true;
        self.park_cursor_at_table(&buffer, table);
        self.recover_table_cover(&buffer);
        self.state.borrow_mut().render.parking = false;
        self.clear_cell_history();
        if let Some(entry) = self.table_entry(table, row, col) {
            entry.set_can_focus(true);
            entry.grab_focus();
            let n = chars_of(&entry);
            self.state.borrow_mut().render.cell_typing = false;
            self.set_cell_range(&entry, n, n);
        }
    }

    fn on_table_cell_click(
        &self,
        table: usize,
        row: usize,
        col: usize,
        x: f64,
        y: f64,
        n_press: i32,
    ) {
        let same = self.focused_table_cell() == Some((table, row, col));
        // Пока видна подпись с переносом, место щелчка считается по ней.
        let shown_index = self
            .table_entry(table, row, col)
            .and_then(|entry| cell_label_index_at(&entry, x, y));
        if !same {
            self.activate_table_cell(table, row, col);
        }
        let Some(entry) = self.table_entry(table, row, col) else {
            return;
        };
        entry.set_can_focus(true);
        entry.grab_focus();
        let index = shown_index.unwrap_or_else(|| cell_index_at_x(&entry, x));
        let typing = n_press >= 2 || same;
        self.state.borrow_mut().render.cell_typing = typing;
        self.set_cell_range(&entry, index, index);
    }

    fn set_cell_range(&self, entry: &gtk4::Entry, caret: i32, anchor: i32) {
        let n = chars_of(entry);
        {
            let mut state = self.state.borrow_mut();
            state.render.cell_caret = caret.clamp(0, n);
            state.render.cell_anchor = anchor.clamp(0, n);
        }
        self.apply_cell_cursor(entry);
    }

    fn apply_cell_cursor(&self, entry: &gtk4::Entry) {
        let (caret, anchor) = {
            let render = &self.state.borrow().render;
            (render.cell_caret, render.cell_anchor)
        };
        let n = chars_of(entry);
        let caret = caret.clamp(0, n);
        let anchor = anchor.clamp(0, n);
        if caret == anchor {
            entry.select_region(caret, caret);
            entry.set_position(caret);
        } else {
            entry.select_region(caret.min(anchor), caret.max(anchor));
            entry.set_position(caret);
        }
        self.sync_cell_caret_bar(entry);
    }

    fn sync_cell_caret_bar(&self, entry: &gtk4::Entry) {
        let Some(overlay) = entry
            .parent()
            .and_then(|parent| parent.downcast::<gtk4::Overlay>().ok())
        else {
            return;
        };
        let Some(bar) = overlay_caret_bar(&overlay) else {
            return;
        };
        let caret = self.state.borrow().render.cell_caret;
        bar.set_visible(true);
        // Текст шире ячейки поле ввода прокручивает, и курсор у правого края.
        // Без ограничения нарисованный курсор уезжал в соседнюю ячейку.
        let limit = (overlay.width() - 10).max(0);
        bar.set_margin_start(cell_caret_pixel_x(entry, caret).clamp(0, limit));
    }

    pub(super) fn deactivate_table_cells(&self) {
        if let Some((table, row, col)) = self.focused_table_cell() {
            self.commit_active_cell(table, row, col);
        }
        for item in &self.state.borrow().render.tables {
            *item.focus_cell.borrow_mut() = None;
        }
        self.paint_cell_active_class();
        self.lock_source_input(false);
        self.focused_view().grab_focus();
    }

    fn commit_active_cell(&self, table: usize, row: usize, col: usize) {
        let Some(path) = self.focused_path() else {
            return;
        };
        let Some(entry) = self.table_entry(table, row, col) else {
            return;
        };
        let text = tables::escape_cell_pipes(&entry.text());
        self.commit_table_cell(path, table, row, col, format!(" {text} "));
    }

    fn table_entry(&self, table: usize, row: usize, col: usize) -> Option<gtk4::Entry> {
        let pane = self.focused_index();
        let cell = {
            let state = self.state.borrow();
            let live = state
                .render
                .tables
                .iter()
                .find(|item| item.index == table)?;
            live.grids[pane]
                .as_ref()?
                .child_at(col as i32, row as i32)?
        };
        ensure_cell_entry(self, &cell, table, row, col)
    }

    fn paint_cell_active_class(&self) {
        let active = self.focused_table_cell();
        let tables: Vec<[Option<gtk4::Grid>; 2]> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| table.grids.clone())
            .collect();
        let indices: Vec<usize> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| table.index)
            .collect();
        for (grids, index) in tables.iter().zip(indices) {
            for grid in grids.iter().flatten() {
                let mut child = grid.first_child();
                while let Some(widget) = child {
                    let next = widget.next_sibling();
                    clear_cell_active(&widget);
                    child = next;
                }
            }
            if let Some((table, row, col)) = active {
                if table == index {
                    for grid in grids.iter().flatten() {
                        if let Some(widget) = grid.child_at(col as i32, row as i32) {
                            mark_cell_active(&widget);
                        }
                    }
                }
            }
        }
    }

    fn handle_table_key(&self, keyval: gdk::Key, modifier: gdk::ModifierType) -> glib::Propagation {
        let Some((table, row, col)) = self.focused_table_cell() else {
            return glib::Propagation::Proceed;
        };
        let ctrl = modifier.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gdk::ModifierType::SHIFT_MASK);
        if ctrl && (keyval == gdk::Key::z || keyval == gdk::Key::Z) {
            if shift {
                self.redo_cell_text();
            } else {
                self.undo_cell_text();
            }
            return glib::Propagation::Stop;
        }
        if ctrl && (keyval == gdk::Key::y || keyval == gdk::Key::Y) {
            self.redo_cell_text();
            return glib::Propagation::Stop;
        }
        if keyval == gdk::Key::Escape {
            self.deactivate_table_cells();
            return glib::Propagation::Stop;
        }
        let Some(path) = self.focused_path() else {
            return glib::Propagation::Stop;
        };
        let Some(grid) = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .find(|item| item.index == table)
            .and_then(|item| item.grids[self.focused_index()].clone())
        else {
            return glib::Propagation::Stop;
        };
        let cols = grid_columns(&grid);
        let rows = grid_rows(&grid);
        if keyval == gdk::Key::Tab {
            let (nr, nc) = if col + 1 < cols {
                (row, col + 1)
            } else if row + 1 < rows {
                (row + 1, 0)
            } else {
                self.table_add_row(path, table);
                return glib::Propagation::Stop;
            };
            self.activate_table_cell(table, nr, nc);
            return glib::Propagation::Stop;
        }
        if keyval == gdk::Key::Return || keyval == gdk::Key::KP_Enter {
            if row + 1 < rows {
                self.activate_table_cell(table, row + 1, col);
            } else {
                self.table_add_row(path, table);
            }
            return glib::Propagation::Stop;
        }
        if keyval == gdk::Key::F2 {
            let Some(entry) = self.table_entry(table, row, col) else {
                return glib::Propagation::Stop;
            };
            let n = chars_of(&entry);
            self.state.borrow_mut().render.cell_typing = true;
            self.set_cell_range(&entry, n, n);
            return glib::Propagation::Stop;
        }
        let typing = self.state.borrow().render.cell_typing;
        if !typing
            && ((!shift
                && matches!(
                    keyval,
                    gdk::Key::Left
                        | gdk::Key::Right
                        | gdk::Key::Up
                        | gdk::Key::Down
                        | gdk::Key::Home
                        | gdk::Key::End
                ))
                || matches!(keyval, gdk::Key::Up | gdk::Key::Down))
        {
            let (drow, dcol) = match keyval {
                gdk::Key::Left | gdk::Key::Home => (0, -1),
                gdk::Key::Right | gdk::Key::End => (0, 1),
                gdk::Key::Up => (-1, 0),
                gdk::Key::Down => (1, 0),
                _ => (0, 0),
            };
            self.move_table_cell(path, table, row, col, rows, cols, drow, dcol);
            return glib::Propagation::Stop;
        }
        if typing && keyval == gdk::Key::Up {
            self.move_table_cell(path, table, row, col, rows, cols, -1, 0);
            return glib::Propagation::Stop;
        }
        if typing && keyval == gdk::Key::Down {
            self.move_table_cell(path, table, row, col, rows, cols, 1, 0);
            return glib::Propagation::Stop;
        }
        if ctrl
            && !(keyval == gdk::Key::a
                || keyval == gdk::Key::A
                || keyval == gdk::Key::Left
                || keyval == gdk::Key::Right)
        {
            return glib::Propagation::Proceed;
        }
        let Some(entry) = self.table_entry(table, row, col) else {
            return glib::Propagation::Stop;
        };
        self.feed_cell_key(&entry, keyval, modifier);
        glib::Propagation::Stop
    }

    fn move_table_cell(
        &self,
        path: PathBuf,
        table: usize,
        row: usize,
        col: usize,
        rows: usize,
        cols: usize,
        drow: i32,
        dcol: i32,
    ) {
        let nr = row as i32 + drow;
        let nc = col as i32 + dcol;
        if nr < 0 {
            self.deactivate_table_cells();
            self.place_cursor_near_table(table, true);
            return;
        }
        if nr >= rows as i32 {
            self.table_add_row(path, table);
            return;
        }
        if nc < 0 {
            if nr > 0 {
                self.activate_table_cell(table, (nr - 1) as usize, cols.saturating_sub(1));
            } else {
                self.deactivate_table_cells();
                self.place_cursor_near_table(table, true);
            }
            return;
        }
        if nc >= cols as i32 {
            if (nr as usize) + 1 < rows {
                self.activate_table_cell(table, (nr as usize) + 1, 0);
            } else {
                self.table_add_row(path, table);
            }
            return;
        }
        self.activate_table_cell(table, nr as usize, nc as usize);
    }

    fn feed_cell_key(&self, entry: &gtk4::Entry, keyval: gdk::Key, modifier: gdk::ModifierType) {
        let ctrl = modifier.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gdk::ModifierType::SHIFT_MASK);
        let text = entry.text().to_string();
        let n = chars_of(entry);
        let (caret, anchor, typing) = {
            let render = &self.state.borrow().render;
            (
                render.cell_caret.clamp(0, n),
                render.cell_anchor.clamp(0, n),
                render.cell_typing,
            )
        };
        if ctrl && (keyval == gdk::Key::a || keyval == gdk::Key::A) {
            self.state.borrow_mut().render.cell_typing = true;
            self.set_cell_range(entry, n, 0);
            return;
        }
        if matches!(
            keyval,
            gdk::Key::Left | gdk::Key::Right | gdk::Key::Home | gdk::Key::End
        ) {
            let target = match keyval {
                gdk::Key::Left if ctrl => skip_word(&text, caret, -1),
                gdk::Key::Left => (caret - 1).max(0),
                gdk::Key::Right if ctrl => skip_word(&text, caret, 1),
                gdk::Key::Right => (caret + 1).min(n),
                gdk::Key::Home => 0,
                gdk::Key::End => n,
                _ => caret,
            };
            let new_anchor = if shift { anchor } else { target };
            self.state.borrow_mut().render.cell_typing = true;
            self.set_cell_range(entry, target, new_anchor);
            return;
        }
        if keyval == gdk::Key::BackSpace {
            let (start, end) = if let Some(span) = text_selection(typing, caret, anchor) {
                span
            } else if caret > 0 {
                (caret - 1, caret)
            } else {
                self.apply_cell_cursor(entry);
                return;
            };
            self.remember_cell_text(&text);
            let (new_text, pos) = splice_chars(&text, start, end, "");
            entry.set_text(&new_text);
            self.state.borrow_mut().render.cell_typing = true;
            self.set_cell_range(entry, pos, pos);
            return;
        }
        if keyval == gdk::Key::Delete {
            let (start, end) = if let Some(span) = text_selection(typing, caret, anchor) {
                span
            } else if caret < n {
                (caret, caret + 1)
            } else {
                self.apply_cell_cursor(entry);
                return;
            };
            self.remember_cell_text(&text);
            let (new_text, pos) = splice_chars(&text, start, end, "");
            entry.set_text(&new_text);
            self.state.borrow_mut().render.cell_typing = true;
            self.set_cell_range(entry, pos, pos);
            return;
        }
        let Some(ch) = keyval.to_unicode() else {
            return;
        };
        if ch.is_control() {
            return;
        }
        let (start, end) = text_selection(typing, caret, anchor).unwrap_or((caret, caret));
        self.remember_cell_text(&text);
        let (new_text, pos) = splice_chars(&text, start, end, &ch.to_string());
        entry.set_text(&new_text);
        self.state.borrow_mut().render.cell_typing = true;
        self.set_cell_range(entry, pos, pos);
    }

    fn clear_cell_history(&self) {
        let mut state = self.state.borrow_mut();
        state.render.cell_undo.clear();
        state.render.cell_redo.clear();
    }

    fn remember_cell_text(&self, text: &str) {
        let (caret, anchor) = {
            let render = &self.state.borrow().render;
            (render.cell_caret, render.cell_anchor)
        };
        let mut state = self.state.borrow_mut();
        state.render.cell_redo.clear();
        state
            .render
            .cell_undo
            .push((text.to_string(), caret, anchor));
        if state.render.cell_undo.len() > 200 {
            state.render.cell_undo.remove(0);
        }
    }

    pub(super) fn undo_cell_text(&self) {
        self.step_cell_history(true);
    }

    pub(super) fn redo_cell_text(&self) {
        self.step_cell_history(false);
    }

    fn step_cell_history(&self, undo: bool) {
        let Some((table, row, col)) = self.focused_table_cell() else {
            return;
        };
        let Some(entry) = self.table_entry(table, row, col) else {
            return;
        };
        let current = (
            entry.text().to_string(),
            self.state.borrow().render.cell_caret,
            self.state.borrow().render.cell_anchor,
        );
        let restored = {
            let mut state = self.state.borrow_mut();
            let render = &mut state.render;
            let item = if undo {
                render.cell_undo.pop()
            } else {
                render.cell_redo.pop()
            };
            let Some(item) = item else {
                return;
            };
            if undo {
                render.cell_redo.push(current);
            } else {
                render.cell_undo.push(current);
            }
            item
        };
        entry.set_text(&restored.0);
        self.state.borrow_mut().render.cell_typing = true;
        self.set_cell_range(&entry, restored.1, restored.2);
    }

    fn ensure_render_tags(&self, buffer: &sourceview5::Buffer) {
        let table = buffer.tag_table();
        ensure_tag(&table, TAG_STRONG, |tag| {
            tag.set_weight(700);
        });
        ensure_tag(&table, TAG_EM, |tag| {
            tag.set_style(pango::Style::Italic);
        });
        ensure_tag(&table, TAG_CODE, |tag| {
            tag.set_family(Some("monospace"));
        });
        ensure_tag(&table, TAG_CODEBLOCK, |tag| {
            tag.set_family(Some("monospace"));
        });
        ensure_tag(&table, TAG_HIDE, hide_source);
        ensure_tag(&table, TAG_TABLE, hide_table_source);
        ensure_tag(&table, TAG_ZOOM, |tag| {
            tag.set_scale(zoom::webkit_factor(self.state.borrow().session.text_pt));
        });
        ensure_tag(&table, TAG_LINK, |tag| {
            tag.set_underline(pango::Underline::Single);
        });
        ensure_tag(&table, TAG_MONO, |tag| {
            tag.set_family(Some("monospace"));
        });
        let scales = [1.6, 1.4, 1.25, 1.15, 1.1, 1.05];
        for (name, scale) in TAG_HEADING.iter().zip(scales) {
            ensure_tag(&table, name, |tag| {
                tag.set_weight(700);
                tag.set_scale(scale);
            });
        }
        if let Some(tag) = table.lookup(TAG_TABLE) {
            raise_tag(&table, &tag);
        }
        if let Some(tag) = table.lookup(TAG_HIDE) {
            raise_tag(&table, &tag);
        }
    }

    /// Раньше подключённые буферы запоминались по адресу локальной ссылки
    /// (`buffer as *const _`), то есть по адресу на стеке. Он совпадал у разных
    /// буферов, и курсор раскрывал разметку только в первом раскрашенном файле.
    fn ensure_cursor_hook(&self, path: &Path, buffer: &sourceview5::Buffer) {
        {
            let mut state = self.state.borrow_mut();
            let Some(entry) = state.buffers.get_mut(path) else {
                return;
            };
            if entry.render_hooked || &entry.buffer != buffer {
                return;
            }
            entry.render_hooked = true;
        }
        let ctx = self.clone();
        buffer.connect_insert_text(move |buffer, _, _| {
            if ctx.state.borrow().render.writing {
                return;
            }
            if ctx.any_table_cell_focused() {
                glib::signal::signal_stop_emission_by_name(buffer, "insert-text");
            }
        });
        let ctx = self.clone();
        buffer.connect_delete_range(move |buffer, _, _| {
            if ctx.state.borrow().render.writing {
                return;
            }
            if ctx.any_table_cell_focused() {
                glib::signal::signal_stop_emission_by_name(buffer, "delete-range");
            }
        });
        let ctx = self.clone();
        buffer.connect_mark_set(move |buf, iter, mark| {
            if mark.name().as_deref() != Some("insert") {
                return;
            }
            if !ctx.editing_markdown() {
                return;
            }
            if ctx.state.borrow().render.writing
                || ctx.state.borrow().render.parking
                || ctx.state.borrow().render.cursor_suspend
            {
                return;
            }
            if !ctx.render_owns_buffer(buf) {
                return;
            }
            let line = iter.line();
            let hit = ctx
                .state
                .borrow()
                .render
                .tables
                .iter()
                .find(|table| line >= table.start_line && line < table.end_line)
                .map(|table| (table.index, table.start_line));
            if let Some((index, start_line)) = hit {
                ctx.state.borrow_mut().render.parking = true;
                ctx.state.borrow_mut().render.cursor_line = Some(start_line);
                ctx.park_cursor_at_table(buf, index);
                ctx.recover_table_cover(buf);
                ctx.state.borrow_mut().render.parking = false;
                if !ctx.any_table_cell_focused() {
                    ctx.activate_table_cell(index, 0, 0);
                }
                return;
            }
            if ctx.state.borrow().render.cursor_line == Some(line) {
                return;
            }
            ctx.shift_cursor_line(line);
        });
    }

    fn park_cursor_at_table(&self, buffer: &sourceview5::Buffer, table: usize) {
        let Some(anchor) = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .find(|item| item.index == table)
            .map(|item| item.anchor.clone())
        else {
            return;
        };
        if anchor.is_deleted() {
            return;
        }
        let iter = buffer.iter_at_child_anchor(&anchor);
        buffer.place_cursor(&iter);
    }

    fn recover_table_cover(&self, buffer: &sourceview5::Buffer) {
        let _prof = crate::ui::prof::span("recover_table_cover");
        let text = buffer_text_of(buffer);
        // Правка ячейки меняет длину исходника таблицы, а границы живых
        // таблиц до следующей полной перерисовки оставались старыми: тег,
        // прячущий исходник, ложился на старый диапазон, и при переходе Tab
        // в другую ячейку пряталось начало следующего абзаца.
        self.sync_live_table_bounds(&text);
        let tables: Vec<(usize, usize)> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| (table.start, table.end))
            .collect();
        let table_anchors: Vec<_> = self
            .state
            .borrow()
            .render
            .tables
            .iter()
            .map(|table| table.anchor.clone())
            .collect();
        let image_anchors: Vec<_> = self
            .state
            .borrow()
            .render
            .images
            .iter()
            .map(|image| image.anchor.clone())
            .collect();
        let map = TextMap::new(buffer, &text);
        for (start, end) in tables {
            cover_table_source(buffer, &text, &map, start, end);
        }
        for anchor in &table_anchors {
            lift_anchor_scale(buffer, anchor);
        }
        for anchor in &image_anchors {
            lift_anchor_scale(buffer, anchor);
        }
    }

    fn sync_live_table_bounds(&self, text: &str) {
        let parsed = tables::parse_tables(text);
        let mut state = self.state.borrow_mut();
        if state.render.tables.len() != parsed.len() {
            return;
        }
        for (live, table) in state.render.tables.iter_mut().zip(parsed.iter()) {
            live.start = table.start;
            live.end = table.end;
            live.start_line = newline_count(slice_bytes(text, 0, table.start));
            let end_line = newline_count(slice_bytes(text, 0, table.end));
            live.end_line = end_line.max(live.start_line + 1);
        }
    }

    fn shift_cursor_line(&self, line: i32) {
        let _prof = crate::ui::prof::span("shift_cursor_line");
        let prev = self.state.borrow().render.cursor_line;
        if prev == Some(line) {
            return;
        }
        let Some(path) = self.focused_path() else {
            return;
        };
        if !self.render_owns(&path) {
            return;
        }
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        let spans = std::mem::take(&mut self.state.borrow_mut().render.spans);
        let line_starts = self.state.borrow().render.line_starts.clone();
        let text_len = self.state.borrow().render.text_len;
        self.state.borrow_mut().render.cursor_line = Some(line);
        let mut fresh = true;
        if let Some(prev) = prev {
            if !line_in_live_table(&self.state.borrow().render, prev) {
                fresh &= retag_cursor_line(&buffer, &spans, &line_starts, text_len, prev, false);
            }
        }
        if !line_in_live_table(&self.state.borrow().render, line) {
            fresh &= retag_cursor_line(&buffer, &spans, &line_starts, text_len, line, true);
        }
        self.state.borrow_mut().render.spans = spans;
        if !fresh {
            self.schedule_render();
        }
    }

    fn rebuild_widgets(&self, buffer: &sourceview5::Buffer, path: &Path, cursor_line: i32) {
        let _prof = crate::ui::prof::span("rebuild_widgets");
        let text = {
            let _p = crate::ui::prof::span("rw:buffer_text");
            buffer_text_of(buffer)
        };
        let spans = {
            let _p = crate::ui::prof::span("rw:mark_spans");
            mark_spans(&text)
        };
        let md_tables = {
            let _p = crate::ui::prof::span("rw:parse_tables");
            tables::parse_tables(&text)
        };
        let bodies: Vec<String> = md_tables
            .iter()
            .map(|table| slice_bytes(&text, table.start, table.end).to_string())
            .collect();
        let image_keys: Vec<String> = spans
            .iter()
            .filter_map(|span| match &span.kind {
                MarkKind::Image { dest } => Some(format!(
                    "{dest}\n{}",
                    slice_bytes(&text, span.start, span.end)
                )),
                _ => None,
            })
            .collect();
        let same_widgets = {
            let state = self.state.borrow();
            state.render.painted
                && state.render.painted_path.as_deref() == Some(path)
                && state.render.table_bodies == bodies
                && state.render.image_keys == image_keys
        } && self.embeds_match_panes(buffer);
        if same_widgets {
            let prev_line = self.state.borrow().render.cursor_line;
            {
                let mut state = self.state.borrow_mut();
                if state.render.tables.len() == md_tables.len() {
                    for (live, table) in state.render.tables.iter_mut().zip(md_tables.iter()) {
                        live.start = table.start;
                        live.end = table.end;
                        live.start_line = newline_count(slice_bytes(&text, 0, table.start));
                        let mut end_line = newline_count(slice_bytes(&text, 0, table.end));
                        if end_line <= live.start_line {
                            end_line = live.start_line + 1;
                        }
                        live.end_line = end_line;
                    }
                }
            }
            let old_sigs = self.state.borrow().render.line_sigs.clone();
            self.store_paint(path, &text, spans, cursor_line, bodies, image_keys);
            let (spans, line_starts, text_len, new_sigs) = {
                let render = &self.state.borrow().render;
                (
                    render.spans.clone(),
                    render.line_starts.clone(),
                    render.text_len,
                    render.line_sigs.clone(),
                )
            };
            // Правка могла изменить оформление не только строки курсора:
            // вставка нескольких строк, отмена, открытие блока кода ```.
            // Раньше такие строки оставались со старыми тегами.
            let changed = changed_lines(&old_sigs, &new_sigs);
            if changed.len() > 400 {
                paint_tags(buffer, &text, &spans, cursor_line);
            } else {
                for line in changed {
                    if line != cursor_line
                        && Some(line) != prev_line
                        && !line_in_live_table(&self.state.borrow().render, line)
                    {
                        retag_cursor_line(buffer, &spans, &line_starts, text_len, line, false);
                    }
                }
            }
            if let Some(prev) = prev_line {
                if prev != cursor_line && !line_in_live_table(&self.state.borrow().render, prev) {
                    retag_cursor_line(buffer, &spans, &line_starts, text_len, prev, false);
                }
            }
            if !line_in_live_table(&self.state.borrow().render, cursor_line) {
                retag_cursor_line(buffer, &spans, &line_starts, text_len, cursor_line, true);
            }
            cover_embedded_source(buffer, &text, &md_tables, &spans, cursor_line);
            let table_anchors: Vec<_> = self
                .state
                .borrow()
                .render
                .tables
                .iter()
                .map(|table| table.anchor.clone())
                .collect();
            let image_anchors: Vec<_> = self
                .state
                .borrow()
                .render
                .images
                .iter()
                .map(|image| image.anchor.clone())
                .collect();
            for anchor in &table_anchors {
                lift_anchor_scale(buffer, anchor);
            }
            for anchor in &image_anchors {
                lift_anchor_scale(buffer, anchor);
            }
            self.apply_text_zoom_tags();
            self.sync_table_grid_widths();
            return;
        }

        let (old_tables, old_images) = {
            let mut state = self.state.borrow_mut();
            (
                std::mem::take(&mut state.render.tables),
                std::mem::take(&mut state.render.images),
            )
        };
        // Якоря этого файла не удаляем, а отдаём новым таблицам и картинкам:
        // удаление якоря попадает в историю отмены и требует её очистки.
        let mut spare: Vec<gtk4::TextChildAnchor> = Vec::new();
        let (reuse_tables, other_tables): (Vec<_>, Vec<_>) =
            old_tables.iter().cloned().partition(|t| t.path == path);
        let (reuse_images, other_images): (Vec<_>, Vec<_>) =
            old_images.iter().cloned().partition(|i| i.path == path);
        for table in &reuse_tables {
            for grid in table.grids.iter().flatten() {
                detach_child(grid);
            }
            if !table.anchor.is_deleted() {
                spare.push(table.anchor.clone());
            }
        }
        for image in &reuse_images {
            for widget in image.widgets.iter().flatten() {
                detach_child(widget);
            }
            if !image.anchor.is_deleted() {
                spare.push(image.anchor.clone());
            }
        }
        self.drop_embeds(&other_tables, &other_images);
        clear_render_tags(buffer);
        drop(reuse_tables);
        drop(reuse_images);
        let freed = !old_tables.is_empty() || !old_images.is_empty();
        {
            let _p = crate::ui::prof::span("rw:free_old_widgets");
            drop(old_tables);
            drop(old_images);
        }
        if freed {
            // Освобождённые сетки таблиц malloc сам системе не отдаёт.
            glib::idle_add_local_once(super::support::release_freed_memory);
        }

        paint_tags(buffer, &text, &spans, cursor_line);

        let cursor_range = line_byte_range(&text, cursor_line);
        let mut placements: Vec<Placement> = md_tables
            .iter()
            .enumerate()
            .map(|(index, table)| Placement::Table {
                index,
                start: table.start,
            })
            .collect();
        for span in &spans {
            if let MarkKind::Image { dest } = &span.kind {
                if !line_contains(cursor_range, span.start) {
                    placements.push(Placement::Image {
                        start: span.start,
                        dest: dest.clone(),
                    });
                }
            }
        }
        placements.sort_by_key(|item| std::cmp::Reverse(item.start()));

        let mut live_tables = Vec::new();
        let mut live_images = Vec::new();

        // Якоря вставляются с конца текста к началу, поэтому карта, снятая
        // до вставки, верна для каждой следующей (более ранней) позиции.
        let map = TextMap::new(buffer, &text);
        self.state.borrow_mut().render.writing = true;
        for place in placements {
            match place {
                Placement::Table { index, .. } => {
                    let Some(table) = md_tables.get(index) else {
                        continue;
                    };
                    let iter = map.iter(buffer, &text, table.start);
                    let anchor = take_or_create_anchor(buffer, iter, &mut spare);
                    if let Some(live) = self.mount_table(buffer, path, &text, anchor, table, index)
                    {
                        live_tables.push(live);
                    }
                }
                Placement::Image { start, dest, .. } => {
                    let iter = map.iter(buffer, &text, start);
                    let anchor = take_or_create_anchor(buffer, iter, &mut spare);
                    if let Some(image) = self.mount_image(buffer, path, anchor, &dest) {
                        live_images.push(image);
                    }
                }
            }
        }
        // Таблица или картинка исчезла из текста: её якорь больше не нужен.
        remove_anchors(&buffer.clone().upcast::<gtk4::TextBuffer>(), spare);
        self.state.borrow_mut().render.writing = false;
        cover_embedded_source(buffer, &text, &md_tables, &spans, cursor_line);
        lift_embedded(buffer, &live_tables, &live_images);

        live_tables.sort_by_key(|table| table.start);
        {
            let mut state = self.state.borrow_mut();
            state.render.tables = live_tables;
            state.render.images = live_images;
        }
        self.store_paint(path, &text, spans, cursor_line, bodies, image_keys);
        self.apply_text_zoom_tags();
        self.restore_table_focus();
        self.sync_table_grid_widths();
        let ctx = self.clone();
        glib::idle_add_local_once(move || ctx.sync_table_grid_widths());
    }

    fn store_paint(
        &self,
        path: &Path,
        text: &str,
        spans: Vec<MarkSpan>,
        cursor_line: i32,
        bodies: Vec<String>,
        image_keys: Vec<String>,
    ) {
        let _prof = crate::ui::prof::span("store_paint");
        let sigs = line_signatures(text, &spans);
        let mut state = self.state.borrow_mut();
        state.render.line_starts = line_starts_of(text);
        state.render.text_len = text.len();
        state.render.line_sigs = sigs;
        state.render.spans = spans;
        state.render.cursor_line = Some(cursor_line);
        state.render.table_bodies = bodies;
        state.render.image_keys = image_keys;
        state.render.painted = true;
        state.render.painted_path = Some(path.to_path_buf());
    }

    /// Виджеты таблиц и картинок стоят ровно в тех полях, что показывают
    /// этот буфер. Иначе (во второй панели открыли тот же файл, или в ней
    /// теперь другой) их надо пересобрать.
    fn embeds_match_panes(&self, buffer: &sourceview5::Buffer) -> bool {
        let split = self.state.borrow().session.split;
        let wanted = [
            self.pane_shows(0, buffer),
            split && self.pane_shows(1, buffer),
        ];
        let images_wanted = [self.pane_shows(0, buffer), self.pane_shows(1, buffer)];
        let state = self.state.borrow();
        state
            .render
            .tables
            .iter()
            .all(|t| (0..2).all(|p| t.grids[p].is_some() == wanted[p]))
            && state
                .render
                .images
                .iter()
                .all(|i| (0..2).all(|p| i.widgets[p].is_some() == images_wanted[p]))
    }

    /// Поле `pane` показывает этот буфер. Якорь чужого буфера в поле
    /// с другим файлом — то же падение GTK, что и раньше.
    fn pane_shows(&self, pane: usize, buffer: &sourceview5::Buffer) -> bool {
        self.widgets.views[pane].buffer() == *buffer.upcast_ref::<gtk4::TextBuffer>()
    }

    fn mount_table(
        &self,
        buffer: &sourceview5::Buffer,
        path: &Path,
        text: &str,
        anchor: gtk4::TextChildAnchor,
        table: &MdTable,
        index: usize,
    ) -> Option<LiveTable> {
        let _prof = crate::ui::prof::span("mount_table");
        lift_anchor_scale(buffer, &anchor);
        let focus_cell = Rc::new(RefCell::new(None));
        // Скрытой второй панели сетка не нужна: каждая ячейка — это Entry,
        // Overlay и три контроллера, а вторая копия удваивала их число.
        let split = self.state.borrow().session.split;
        let wanted = [
            self.pane_shows(0, buffer),
            split && self.pane_shows(1, buffer),
        ];
        let grids = [
            wanted[0].then(|| build_table_grid(self, path, table, index)),
            wanted[1].then(|| build_table_grid(self, path, table, index)),
        ];
        for (pane, grid) in grids.iter().enumerate() {
            let Some(grid) = grid else {
                continue;
            };
            self.widgets.views[pane].add_child_at_anchor(grid, &anchor);
            size_table_grid(grid, &self.widgets.views[pane]);
        }
        let start_line = newline_count(slice_bytes(text, 0, table.start));
        let mut end_line = newline_count(slice_bytes(text, 0, table.end));
        if end_line <= start_line {
            end_line = start_line + 1;
        }
        Some(LiveTable {
            path: path.to_path_buf(),
            index,
            start: table.start,
            end: table.end,
            start_line,
            end_line,
            anchor,
            grids,
            focus_cell,
        })
    }

    fn mount_image(
        &self,
        buffer: &sourceview5::Buffer,
        path: &Path,
        anchor: gtk4::TextChildAnchor,
        dest: &str,
    ) -> Option<LiveImage> {
        let _prof = crate::ui::prof::span("mount_image");
        lift_anchor_scale(buffer, &anchor);
        let file = resolve_image(path, dest);
        let mut widgets = [None, None];
        for (pane, slot) in widgets.iter_mut().enumerate() {
            if self.pane_shows(pane, buffer) {
                let widget = image_widget(file.as_deref());
                self.widgets.views[pane].add_child_at_anchor(&widget, &anchor);
                *slot = Some(widget);
            }
        }
        Some(LiveImage {
            path: path.to_path_buf(),
            anchor,
            widgets,
        })
    }

    fn place_cursor_near_table(&self, table: usize, before: bool) {
        let Some(path) = self.focused_path() else {
            return;
        };
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        let text = buffer_text_of(&buffer);
        let Some(item) = tables::parse_tables(&text).get(table).cloned() else {
            return;
        };
        let byte = if before {
            item.start.saturating_sub(1)
        } else {
            item.end.min(text.len())
        };
        let iter = iter_at_file_byte(&buffer, &text, byte);
        buffer.place_cursor(&iter);
        self.focused_view().grab_focus();
    }
}

enum Placement {
    Table { index: usize, start: usize },
    Image { start: usize, dest: String },
}

impl Placement {
    fn start(&self) -> usize {
        match self {
            Self::Table { start, .. } | Self::Image { start, .. } => *start,
        }
    }
}

fn build_table_grid(ctx: &Ctx, path: &Path, table: &MdTable, table_index: usize) -> gtk4::Grid {
    let _prof = crate::ui::prof::span("build_table_grid");
    let grid = gtk4::Grid::new();
    grid.add_css_class("tw-md-table");
    // Ширина колонок по содержимому: длинному тексту больше места.
    // Таблица всё равно не шире редактора (size_table_grid).
    grid.set_column_homogeneous(false);
    grid.set_column_spacing(0);
    grid.set_row_spacing(0);
    grid.set_hexpand(true);
    grid.set_halign(gtk4::Align::Fill);
    grid.set_can_target(true);

    let rows = table.row_count();
    let cols = table.column_count();
    for row in 0..rows {
        for col in 0..cols {
            let raw = table
                .cell(row, col)
                .map(|cell| cell.text.clone())
                .unwrap_or_default();
            let xalign = match table.alignments.get(col).copied().unwrap_or(ColAlign::None) {
                ColAlign::Left | ColAlign::None => 0.0,
                ColAlign::Center => 0.5,
                ColAlign::Right => 1.0,
            };
            // Вне правки ячейка показывает текст подписью с переносом по ширине
            // колонки. Однострочное поле ввода видно только в активной ячейке.
            // В файл при этом ничего не добавляется: перенос только на экране.
            let label = gtk4::Label::new(Some(raw.trim()));
            label.set_wrap(true);
            label.set_wrap_mode(pango::WrapMode::WordChar);
            label.set_max_width_chars(30);
            // Pango при разрыве слова сам дописывает дефис. В файле его нет,
            // поэтому и на экране он не нужен.
            let attrs = pango::AttrList::new();
            attrs.insert(pango::AttrInt::new_insert_hyphens(false));
            label.set_attributes(Some(&attrs));
            label.set_xalign(xalign);
            label.set_hexpand(true);
            label.set_halign(gtk4::Align::Fill);
            label.set_valign(gtk4::Align::Fill);
            label.add_css_class("tw-md-cell-label");
            if row == 0 {
                label.add_css_class("tw-md-cell-head");
            }
            // Поле ввода и курсор появятся при первой правке (ensure_cell_entry)
            // поверх подписи; размер ячейки всегда задаёт подпись.

            let overlay = gtk4::Overlay::new();
            // Щелчки ловит вся ячейка: вне правки поле ввода скрыто.
            let ctx_cell = ctx.clone();
            let grab = gtk4::GestureClick::new();
            grab.set_button(1);
            grab.set_propagation_phase(gtk4::PropagationPhase::Capture);
            grab.connect_pressed(move |gesture, n_press, x, y| {
                ctx_cell.on_table_cell_click(table_index, row, col, x, y, n_press);
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            });
            overlay.add_controller(grab);
            let ctx_menu = ctx.clone();
            let path_menu = path.to_path_buf();
            let right = gtk4::GestureClick::new();
            right.set_button(3);
            right.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk4::EventSequenceState::Claimed);
                // Виджет берётся у жеста. Сильная ссылка на ячейку внутри её же
                // обработчика замыкала круг: ячейка держит жест, жест — ячейку,
                // и сетки таблиц не освобождались после ухода с файла.
                let Some(anchor) = gesture.widget() else {
                    return;
                };
                ctx_menu.show_table_menu(&anchor, path_menu.clone(), table_index, row, col);
            });
            overlay.add_controller(right);
            overlay.set_hexpand(true);
            overlay.set_halign(gtk4::Align::Fill);
            overlay.set_child(Some(&label));
            grid.attach(&overlay, col as i32, row as i32, 1, 1);
        }
    }
    grid
}

/// Поле ввода и курсор ячейки. Создаются при первой правке этой ячейки:
/// GtkEntry тяжёлый, а нужен только в одной ячейке за раз.
fn ensure_cell_entry(
    ctx: &Ctx,
    cell: &gtk4::Widget,
    table_index: usize,
    row: usize,
    col: usize,
) -> Option<gtk4::Entry> {
    if let Some(entry) = cell_entry_of(cell) {
        return Some(entry);
    }
    let overlay = cell.clone().downcast::<gtk4::Overlay>().ok()?;
    let label = cell_label_of(cell)?;
    let entry = gtk4::Entry::new();
    entry.set_visible(false);
    entry.set_text(&label.text());
    entry.set_hexpand(true);
    entry.set_halign(gtk4::Align::Fill);
    // Узкое поле: ширину колонки задаёт таблица, а не текст. Размер ячейки
    // задаёт подпись, поэтому при правке таблица не меняет размеры.
    entry.set_width_chars(1);
    entry.set_valign(gtk4::Align::Center);
    entry.set_editable(true);
    entry.set_can_focus(true);
    entry.set_focus_on_click(false);
    entry.add_css_class("tw-md-cell");
    if label.has_css_class("tw-md-cell-head") {
        entry.add_css_class("tw-md-cell-head");
    }
    gtk4::prelude::EditableExt::set_alignment(&entry, label.xalign());
    let shown = label.clone();
    entry.connect_changed(move |entry| {
        let text = entry.text();
        if shown.text() != text {
            shown.set_text(&text);
        }
    });
    let ctx_alloc = ctx.clone();
    entry.connect_notify_local(Some("width"), move |entry, _| {
        if ctx_alloc.focused_table_cell() == Some((table_index, row, col)) {
            ctx_alloc.sync_cell_caret_bar(entry);
        }
    });
    overlay.add_overlay(&entry);
    let bar = gtk4::DrawingArea::new();
    bar.add_css_class("tw-md-caret");
    bar.set_halign(gtk4::Align::Start);
    // Высота курсора — строка шрифта, а не ячейка: с переносом текста строки
    // таблицы разной высоты, и курсор растягивался на всю ячейку.
    // По центру, как и однострочное поле ввода (valign Center).
    bar.set_valign(gtk4::Align::Center);
    bar.set_content_height(ctx.cell_caret_height(&entry));
    bar.set_can_target(false);
    bar.set_can_focus(false);
    bar.set_sensitive(false);
    bar.set_visible(false);
    bar.set_content_width(2);
    bar.set_draw_func(|area, cr, width, height| {
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        cr.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
        let _ = cr.fill();
    });
    overlay.add_overlay(&bar);
    Some(entry)
}

fn paint_tags(buffer: &sourceview5::Buffer, text: &str, spans: &[MarkSpan], cursor_line: i32) {
    let _prof = crate::ui::prof::span("paint_tags");
    clear_render_tags(buffer);
    let cursor_range = line_byte_range(text, cursor_line);
    let mut jobs: Vec<(usize, usize, &'static str)> = Vec::new();
    for span in spans {
        let on_cursor = line_contains(cursor_range, span.start)
            || cursor_range.is_some_and(|(a, b)| span.start < b && span.end > a);
        match &span.kind {
            MarkKind::Heading { level } => {
                let name = TAG_HEADING
                    .get((*level as usize).saturating_sub(1))
                    .copied()
                    .unwrap_or("tw-h6");
                jobs.push((span.start, span.end, name));
            }
            MarkKind::HeadingMark | MarkKind::Delimiter | MarkKind::LinkMarkup => {
                if !on_cursor {
                    jobs.push((span.start, span.end, TAG_HIDE));
                }
            }
            MarkKind::Strong => jobs.push((span.start, span.end, TAG_STRONG)),
            MarkKind::Emphasis => jobs.push((span.start, span.end, TAG_EM)),
            MarkKind::Code => {
                jobs.push((span.start, span.end, TAG_CODE));
                jobs.push((span.start, span.end, TAG_MONO));
            }
            MarkKind::CodeBlock => {
                jobs.push((span.start, span.end, TAG_CODEBLOCK));
                jobs.push((span.start, span.end, TAG_MONO));
            }
            MarkKind::Link { .. } | MarkKind::WikiLink { .. } => {
                jobs.push((span.start, span.end, TAG_LINK))
            }
            MarkKind::Image { .. } => {}
        }
    }
    jobs.sort_by_key(|job| job.0);
    let mut walk = ByteCursor::new(buffer, text);
    for (start, end, name) in jobs {
        walk.apply(buffer, start, end, name);
    }
}

/// Исходник таблицы и картинки прячется уже после якоря, иначе GtkGrid
/// наследует scale 0.001 и рисуется крошечным.
fn cover_embedded_source(
    buffer: &sourceview5::Buffer,
    text: &str,
    tables: &[MdTable],
    spans: &[MarkSpan],
    cursor_line: i32,
) {
    let _prof = crate::ui::prof::span("cover_embedded_source");
    let map = TextMap::new(buffer, text);
    for table in tables {
        cover_table_source(buffer, text, &map, table.start, table.end);
    }
    let cursor_range = line_byte_range(text, cursor_line);
    let mut jobs: Vec<(usize, usize, &'static str)> = Vec::new();
    for span in spans {
        if let MarkKind::Image { .. } = &span.kind {
            let on_cursor = line_contains(cursor_range, span.start)
                || cursor_range.is_some_and(|(a, b)| span.start < b && span.end > a);
            if !on_cursor {
                jobs.push((span.start, span.end, TAG_HIDE));
            }
        }
    }
    jobs.sort_by_key(|job| job.0);
    let mut walk = ByteCursor { text, map };
    for (start, end, name) in jobs {
        walk.apply(buffer, start, end, name);
    }
}

fn cover_table_source(
    buffer: &sourceview5::Buffer,
    text: &str,
    map: &TextMap,
    start: usize,
    end: usize,
) {
    let _prof = crate::ui::prof::span("cover_table_source");
    let mut from = map.iter(buffer, text, start);
    let to = map.iter(buffer, text, end);
    if from.child_anchor().is_some() {
        from.forward_char();
    }
    if from.offset() >= to.offset() {
        return;
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_TABLE) {
        buffer.apply_tag(&tag, &from, &to);
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_ZOOM) {
        buffer.remove_tag(&tag, &from, &to);
    }
}

fn lift_anchor_scale(buffer: &sourceview5::Buffer, anchor: &gtk4::TextChildAnchor) {
    let _prof = crate::ui::prof::span("lift_anchor_scale");
    let a = buffer.iter_at_child_anchor(anchor);
    let mut b = a;
    if !b.forward_char() {
        return;
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_TABLE) {
        buffer.remove_tag(&tag, &a, &b);
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_HIDE) {
        buffer.remove_tag(&tag, &a, &b);
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_ZOOM) {
        buffer.remove_tag(&tag, &a, &b);
    }
}

fn lift_embedded(buffer: &sourceview5::Buffer, tables: &[LiveTable], images: &[LiveImage]) {
    let _prof = crate::ui::prof::span("lift_embedded");
    for table in tables {
        lift_anchor_scale(buffer, &table.anchor);
    }
    for image in images {
        lift_anchor_scale(buffer, &image.anchor);
    }
}

fn retag_cursor_line(
    buffer: &sourceview5::Buffer,
    spans: &[MarkSpan],
    line_starts: &[usize],
    text_len: usize,
    line: i32,
    on_cursor: bool,
) -> bool {
    let _prof = crate::ui::prof::span("retag_cursor_line");
    if line < 0 {
        return true;
    }
    let Some(&start) = line_starts.get(line as usize) else {
        return true;
    };
    let end = line_starts
        .get(line as usize + 1)
        .copied()
        .unwrap_or(text_len);
    let Some(from) = buffer.iter_at_line(line) else {
        return true;
    };
    let mut to = from;
    if !to.forward_line() {
        to = buffer.end_iter();
    }
    let raw = buffer.slice(&from, &to, true);
    let line_text = raw.replace('\u{FFFC}', "");
    if line_text.len() != end.saturating_sub(start) {
        return false;
    }
    let table = buffer.tag_table();
    for name in [
        TAG_HIDE,
        TAG_MONO,
        TAG_STRONG,
        TAG_EM,
        TAG_CODE,
        TAG_CODEBLOCK,
        TAG_LINK,
    ] {
        if let Some(tag) = table.lookup(name) {
            buffer.remove_tag(&tag, &from, &to);
        }
    }
    for name in TAG_HEADING {
        if let Some(tag) = table.lookup(name) {
            buffer.remove_tag(&tag, &from, &to);
        }
    }
    let mut pieces: Vec<(usize, usize, &'static str)> = Vec::new();
    for span in spans {
        if span.end <= start || span.start >= end {
            continue;
        }
        let piece_start = span.start.max(start);
        let piece_end = span.end.min(end);
        match &span.kind {
            MarkKind::Heading { level } => {
                let name = TAG_HEADING
                    .get((*level as usize).saturating_sub(1))
                    .copied()
                    .unwrap_or("tw-h6");
                pieces.push((piece_start, piece_end, name));
            }
            MarkKind::HeadingMark | MarkKind::Delimiter | MarkKind::LinkMarkup if !on_cursor => {
                pieces.push((piece_start, piece_end, TAG_HIDE));
            }
            MarkKind::Strong => pieces.push((piece_start, piece_end, TAG_STRONG)),
            MarkKind::Emphasis => pieces.push((piece_start, piece_end, TAG_EM)),
            MarkKind::Code => {
                pieces.push((piece_start, piece_end, TAG_CODE));
                pieces.push((piece_start, piece_end, TAG_MONO));
            }
            MarkKind::CodeBlock => {
                pieces.push((piece_start, piece_end, TAG_CODEBLOCK));
                pieces.push((piece_start, piece_end, TAG_MONO));
            }
            MarkKind::Link { .. } | MarkKind::WikiLink { .. } => {
                pieces.push((piece_start, piece_end, TAG_LINK))
            }
            MarkKind::Image { .. } if !on_cursor => {
                pieces.push((piece_start, piece_end, TAG_HIDE));
            }
            _ => {}
        }
    }
    pieces.sort_by_key(|piece| piece.0);
    let mut iter = from;
    let mut at = start;
    for (piece_start, piece_end, name) in pieces {
        let Some(tag) = table.lookup(name) else {
            continue;
        };
        let a = forward_in_line(&line_text, start, &mut iter, &mut at, piece_start);
        let b = forward_in_line(&line_text, start, &mut iter, &mut at, piece_end);
        buffer.apply_tag(&tag, &a, &b);
    }
    true
}

fn forward_in_line(
    line_text: &str,
    line_start: usize,
    iter: &mut gtk4::TextIter,
    at: &mut usize,
    byte: usize,
) -> gtk4::TextIter {
    if byte > *at {
        let from = byte_floor(line_text, at.saturating_sub(line_start));
        let to = byte_floor(line_text, byte.saturating_sub(line_start));
        if from < to {
            let chars = line_text[from..to].chars().count();
            iter.forward_chars(chars as i32);
        }
        *at = byte;
    }
    *iter
}

/// Байтовые смещения текста файла → позиции в буфере.
/// В буфере есть якоря таблиц и картинок (U+FFFC), в тексте файла их нет.
/// Позиция ищется через `iter_at_offset` (B-дерево GTK), а не шагами по
/// одному символу от начала буфера: раньше вложенные отрезки (заголовок
/// и его `#`, код с двумя тегами, ссылка и её скобки) каждый раз
/// возвращали курсор в начало, и раскраска росла квадратично.
struct TextMap {
    line_bytes: Vec<usize>,
    line_chars: Vec<usize>,
    /// Номер символа текста, перед которым стоит якорь. По возрастанию.
    anchors: Vec<usize>,
}

impl TextMap {
    fn new(buffer: &sourceview5::Buffer, text: &str) -> Self {
        let mut line_bytes = vec![0];
        let mut line_chars = vec![0];
        let mut chars = 0usize;
        for (index, ch) in text.char_indices() {
            chars += 1;
            if ch == '\n' {
                line_bytes.push(index + 1);
                line_chars.push(chars);
            }
        }
        let mut anchors = Vec::new();
        let raw = buffer.slice(&buffer.start_iter(), &buffer.end_iter(), true);
        if raw.contains('\u{FFFC}') {
            for (index, ch) in raw.chars().enumerate() {
                if ch == '\u{FFFC}' {
                    anchors.push(index - anchors.len());
                }
            }
        }
        Self {
            line_bytes,
            line_chars,
            anchors,
        }
    }

    /// Номер символа в тексте файла для байтового смещения.
    fn char_index(&self, text: &str, byte: usize) -> usize {
        let byte = byte_floor(text, byte.min(text.len()));
        let line = self.line_bytes.partition_point(|start| *start <= byte) - 1;
        let start = self.line_bytes[line];
        self.line_chars[line] + text[start..byte].chars().count()
    }

    /// Смещение в буфере. Якоря на этой позиции пропускаются, как раньше.
    fn offset(&self, text: &str, byte: usize) -> i32 {
        let chars = self.char_index(text, byte);
        let before = self.anchors.partition_point(|at| *at <= chars);
        (chars + before) as i32
    }

    fn iter(&self, buffer: &sourceview5::Buffer, text: &str, byte: usize) -> gtk4::TextIter {
        buffer.iter_at_offset(self.offset(text, byte))
    }
}

struct ByteCursor<'a> {
    text: &'a str,
    map: TextMap,
}

impl<'a> ByteCursor<'a> {
    fn new(buffer: &'a sourceview5::Buffer, text: &'a str) -> Self {
        Self {
            text,
            map: TextMap::new(buffer, text),
        }
    }

    fn apply(&mut self, buffer: &sourceview5::Buffer, start: usize, end: usize, name: &str) {
        if start >= end || end > self.text.len() {
            return;
        }
        let Some(tag) = buffer.tag_table().lookup(name) else {
            return;
        };
        let a = self.map.iter(buffer, self.text, start);
        let b = self.map.iter(buffer, self.text, end);
        buffer.apply_tag(&tag, &a, &b);
    }
}

fn clear_render_tags(buffer: &sourceview5::Buffer) {
    let _prof = crate::ui::prof::span("clear_render_tags");
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    for name in [
        TAG_STRONG,
        TAG_EM,
        TAG_CODE,
        TAG_CODEBLOCK,
        TAG_HIDE,
        TAG_TABLE,
        TAG_LINK,
        TAG_MONO,
    ] {
        if let Some(tag) = buffer.tag_table().lookup(name) {
            buffer.remove_tag(&tag, &start, &end);
        }
    }
    for name in TAG_HEADING {
        if let Some(tag) = buffer.tag_table().lookup(name) {
            buffer.remove_tag(&tag, &start, &end);
        }
    }
}

/// Служебный текст скрыт по-настоящему (invisible): не рисуется и не занимает
/// места. Прозрачный цвет GTK не учитывает, символы оставались видны цветом
/// подсветки синтаксиса. Падение при прокрутке, из-за которого от invisible
/// отказывались, давали якоря таблиц из чужого буфера (исправлено отдельно);
/// стресс-прокрутка с правками на invisible падений не даёт. scale≈0 по-прежнему
/// нельзя: ломает кэш строк GTK.
fn hide_source(tag: &gtk4::TextTag) {
    tag.set_scale(1.0);
    tag.set_foreground_rgba(Some(&gtk4::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)));
    tag.set_invisible(true);
}

/// Исходник таблицы прячется без scale≈0: иначе GtkGrid наследует масштаб
/// и превращается в чёрную полосу, а номера строк наезжают друг на друга.
fn hide_table_source(tag: &gtk4::TextTag) {
    tag.set_scale(1.0);
    tag.set_invisible(true);
}

fn table_pixel_width(view: &sourceview5::View) -> i32 {
    let mut width = view.width();
    if width < 80 {
        if let Some(scroll) = scrolled_of(view) {
            width = scroll.width();
        }
    }
    if width < 80 {
        560
    } else {
        (width - 56).clamp(120, 2400)
    }
}

fn size_table_grid(grid: &gtk4::Grid, view: &sourceview5::View) {
    grid.set_size_request(table_pixel_width(view), -1);
    grid.queue_resize();
}

fn line_in_live_table(render: &RenderState, line: i32) -> bool {
    render
        .tables
        .iter()
        .any(|table| line >= table.start_line && line < table.end_line)
}

fn ensure_tag(table: &gtk4::TextTagTable, name: &str, configure: impl FnOnce(&gtk4::TextTag)) {
    if let Some(tag) = table.lookup(name) {
        configure(&tag);
        return;
    }
    let tag = gtk4::TextTag::new(Some(name));
    configure(&tag);
    table.add(&tag);
}

fn raise_tag(table: &gtk4::TextTagTable, tag: &gtk4::TextTag) {
    let last = table.size().saturating_sub(1);
    if last > 0 {
        tag.set_priority(last);
    }
}

/// Отличающийся кусок двух текстов: `old[from..old_to]` заменяется на
/// `new[from..new_to]`. Границы — по символам.
fn changed_span(old: &str, new: &str) -> (usize, usize, usize) {
    let mut from = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(from) || !new.is_char_boundary(from) {
        from -= 1;
    }
    let max_tail = (old.len() - from).min(new.len() - from);
    let mut tail = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_tail)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - tail) || !new.is_char_boundary(new.len() - tail) {
        tail -= 1;
    }
    (from, old.len() - tail, new.len() - tail)
}

fn iter_at_file_byte(buffer: &sourceview5::Buffer, text: &str, byte: usize) -> gtk4::TextIter {
    let _prof = crate::ui::prof::span("iter_at_file_byte");
    TextMap::new(buffer, text).iter(buffer, text, byte)
}

fn file_byte_at_iter(buffer: &gtk4::TextBuffer, iter: &gtk4::TextIter) -> usize {
    let start = buffer.start_iter();
    buffer.text(&start, iter, true).len()
}

/// Для каждой строки: хеш её текста и хеш разметки на ней (вид и границы
/// кусков относительно начала строки).
fn line_signatures(text: &str, spans: &[MarkSpan]) -> Vec<(u64, u64)> {
    use std::hash::{Hash, Hasher};
    let starts = line_starts_of(text);
    let line_end = |line: usize| starts.get(line + 1).copied().unwrap_or(text.len());
    let mut marks: Vec<std::collections::hash_map::DefaultHasher> =
        (0..starts.len()).map(|_| Default::default()).collect();
    for span in spans {
        let mut line = starts
            .partition_point(|at| *at <= span.start)
            .saturating_sub(1);
        while line < starts.len() && starts[line] < span.end.max(span.start + 1) {
            let begin = starts[line];
            let hasher = &mut marks[line];
            std::mem::discriminant(&span.kind).hash(hasher);
            (span.start.max(begin) - begin).hash(hasher);
            (span.end.min(line_end(line)) - begin).hash(hasher);
            line += 1;
        }
    }
    starts
        .iter()
        .enumerate()
        .map(|(line, start)| {
            let mut content = std::collections::hash_map::DefaultHasher::new();
            text[*start..line_end(line)].hash(&mut content);
            (content.finish(), marks[line].finish())
        })
        .collect()
}

/// Строки нового текста, которые надо перекрасить: изменённые правкой
/// (между общими началом и концом) и те, у которых при том же тексте
/// поменялась разметка.
fn changed_lines(old: &[(u64, u64)], new: &[(u64, u64)]) -> Vec<i32> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a.0 == b.0).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a.0 == b.0)
        .count();
    let mut out = Vec::new();
    for line in 0..prefix {
        if old[line].1 != new[line].1 {
            out.push(line as i32);
        }
    }
    for line in prefix..new.len() - suffix {
        out.push(line as i32);
    }
    for k in 1..=suffix {
        if old[old.len() - k].1 != new[new.len() - k].1 {
            out.push((new.len() - k) as i32);
        }
    }
    out
}

fn line_starts_of(text: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index + 1);
        }
    }
    starts
}

fn byte_floor(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn slice_bytes(text: &str, start: usize, end: usize) -> &str {
    let mut start = byte_floor(text, start);
    let end = byte_floor(text, end);
    if start > end {
        start = end;
    }
    &text[start..end]
}

fn newline_count(text: &str) -> i32 {
    text.bytes().filter(|byte| *byte == b'\n').count() as i32
}

fn line_byte_range(text: &str, line: i32) -> Option<(usize, usize)> {
    if line < 0 {
        return None;
    }
    let mut start = 0usize;
    for (index, part) in text.split_inclusive('\n').enumerate() {
        let end = start + part.len();
        if index as i32 == line {
            return Some((start, end));
        }
        start = end;
    }
    None
}

fn line_contains(line: Option<(usize, usize)>, byte: usize) -> bool {
    line.is_some_and(|(a, b)| byte >= a && byte < b)
}

/// Якорь, оставшийся от прошлой отрисовки прямо перед `iter` (карта текста
/// ставит позицию после якорей), или новый.
fn take_or_create_anchor(
    buffer: &sourceview5::Buffer,
    mut iter: gtk4::TextIter,
    spare: &mut Vec<gtk4::TextChildAnchor>,
) -> gtk4::TextChildAnchor {
    let mut prev = iter;
    if prev.backward_char() {
        if let Some(found) = prev.child_anchor() {
            if let Some(at) = spare.iter().position(|a| *a == found) {
                return spare.swap_remove(at);
            }
        }
    }
    buffer.create_child_anchor(&mut iter)
}

/// Удалить якоря вне истории отмены (см. `remove_path_anchors`).
fn remove_anchors(buffer: &gtk4::TextBuffer, anchors: Vec<gtk4::TextChildAnchor>) {
    let mut positions: Vec<(i32, gtk4::TextChildAnchor)> = anchors
        .into_iter()
        .filter(|a| !a.is_deleted())
        .map(|a| (buffer.iter_at_child_anchor(&a).offset(), a))
        .collect();
    if positions.is_empty() {
        return;
    }
    positions.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    buffer.begin_irreversible_action();
    for (_, anchor) in positions {
        if anchor.is_deleted() {
            continue;
        }
        let mut iter = buffer.iter_at_child_anchor(&anchor);
        let mut end = iter;
        end.forward_char();
        buffer.delete(&mut iter, &mut end);
    }
    buffer.end_irreversible_action();
}

fn remove_path_anchors(
    buffer: &gtk4::TextBuffer,
    tables: &[LiveTable],
    images: &[LiveImage],
    path: &Path,
) {
    let _prof = crate::ui::prof::span("remove_path_anchors");
    let mut positions: Vec<(i32, gtk4::TextChildAnchor)> = Vec::new();
    for table in tables.iter().filter(|table| table.path == path) {
        if !table.anchor.is_deleted() {
            positions.push((
                buffer.iter_at_child_anchor(&table.anchor).offset(),
                table.anchor.clone(),
            ));
        }
    }
    for image in images.iter().filter(|image| image.path == path) {
        if !image.anchor.is_deleted() {
            positions.push((
                buffer.iter_at_child_anchor(&image.anchor).offset(),
                image.anchor.clone(),
            ));
        }
    }
    positions.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    if positions.is_empty() {
        return;
    }
    // Вставку якоря GTK в историю отмены не пишет, а удаление пишет как
    // удаление символа U+FFFC. Без этой обёртки Ctrl+Z после перерисовки
    // или возврата в файл вставлял мусорный U+FFFC, и все более ранние шаги
    // отмены попадали со сдвигом — портили текст. Необратимое действие
    // очищает историю: отмена теряется, но текст не портится.
    buffer.begin_irreversible_action();
    for (_, anchor) in positions {
        if anchor.is_deleted() {
            continue;
        }
        let mut iter = buffer.iter_at_child_anchor(&anchor);
        let mut end = iter;
        end.forward_char();
        buffer.delete(&mut iter, &mut end);
    }
    buffer.end_irreversible_action();
}

fn detach_child(widget: &impl IsA<gtk4::Widget>) {
    let _prof = crate::ui::prof::span("detach_child");
    if let Some(parent) = widget.parent() {
        if let Ok(view) = parent.downcast::<gtk4::TextView>() {
            view.remove(widget);
        }
    }
}

fn pick_is_table_cell(view: &impl IsA<gtk4::Widget>, x: f64, y: f64) -> bool {
    let Some(mut widget) = view.pick(x, y, gtk4::PickFlags::DEFAULT) else {
        return false;
    };
    loop {
        if widget.has_css_class("tw-md-cell") || widget.has_css_class("tw-md-table") {
            return true;
        }
        match widget.parent() {
            Some(parent) => widget = parent,
            None => return false,
        }
    }
}

fn cell_entry_of(widget: &gtk4::Widget) -> Option<gtk4::Entry> {
    if let Ok(entry) = widget.clone().downcast::<gtk4::Entry>() {
        return Some(entry);
    }
    let overlay = widget.clone().downcast::<gtk4::Overlay>().ok()?;
    let mut child = overlay.first_child();
    while let Some(node) = child {
        if let Ok(entry) = node.clone().downcast::<gtk4::Entry>() {
            return Some(entry);
        }
        child = node.next_sibling();
    }
    None
}

/// Подпись ячейки: основной ребёнок Overlay.
fn cell_label_of(widget: &gtk4::Widget) -> Option<gtk4::Label> {
    widget
        .clone()
        .downcast::<gtk4::Overlay>()
        .ok()?
        .child()?
        .downcast::<gtk4::Label>()
        .ok()
}

/// Ячейка показывает поле ввода (правка) поверх подписи или только подпись.
/// Подпись при правке прозрачная, но остаётся на месте и держит размер.
fn show_cell_editor(widget: &gtk4::Widget, editing: bool) {
    let Some(entry) = cell_entry_of(widget) else {
        return;
    };
    if gtk4::prelude::WidgetExt::is_visible(&entry) != editing {
        entry.set_visible(editing);
    }
    if let Some(label) = cell_label_of(widget) {
        label.set_opacity(if editing { 0.0 } else { 1.0 });
    }
}

/// Номер символа под щелчком по подписи ячейки, с учётом переноса строк.
/// None, если ячейка уже в правке.
fn cell_label_index_at(entry: &gtk4::Entry, x: f64, y: f64) -> Option<i32> {
    if gtk4::prelude::WidgetExt::is_visible(entry) {
        return None;
    }
    let overlay = entry.parent()?;
    let label = cell_label_of(&overlay)?;
    let text = label.text().to_string();
    if text.is_empty() {
        return Some(0);
    }
    let (ox, oy) = label.layout_offsets();
    let px = ((x - f64::from(ox)).max(0.0) * f64::from(pango::SCALE)) as i32;
    let py = ((y - f64::from(oy)).max(0.0) * f64::from(pango::SCALE)) as i32;
    let (_, byte_index, trailing) = label.layout().xy_to_index(px, py);
    let mut byte_index = (byte_index as usize).min(text.len());
    while byte_index > 0 && !text.is_char_boundary(byte_index) {
        byte_index -= 1;
    }
    let chars = text[..byte_index].chars().count() as i32 + trailing;
    Some(chars.min(text.chars().count() as i32))
}

fn overlay_caret_bar(overlay: &gtk4::Overlay) -> Option<gtk4::Widget> {
    let mut child = overlay.first_child();
    while let Some(widget) = child {
        let next = widget.next_sibling();
        if widget.has_css_class("tw-md-caret") {
            return Some(widget);
        }
        child = next;
    }
    None
}

fn clear_cell_active(widget: &gtk4::Widget) {
    widget.remove_css_class("tw-md-cell-active");
    show_cell_editor(widget, false);
    if let Ok(overlay) = widget.clone().downcast::<gtk4::Overlay>() {
        if let Some(bar) = overlay_caret_bar(&overlay) {
            bar.set_visible(false);
        }
        if let Some(child) = overlay.child() {
            child.remove_css_class("tw-md-cell-active");
        }
    }
}

fn mark_cell_active(widget: &gtk4::Widget) {
    widget.add_css_class("tw-md-cell-active");
    show_cell_editor(widget, true);
    if let Some(entry) = cell_entry_of(widget) {
        entry.add_css_class("tw-md-cell-active");
    }
}

/// Высота строки шрифта ячейки при размере текста `pt`, в пикселях.
/// Размер берётся из настроек, а не из стиля поля: сразу после смены
/// масштаба новый CSS к полю ещё не применён.
fn measure_line_height(entry: &gtk4::Entry, pt: i32) -> i32 {
    let context = entry.pango_context();
    let mut font = context.font_description().unwrap_or_default();
    font.set_size(zoom::clamp_pt(pt) * pango::SCALE);
    let layout = pango::Layout::new(&context);
    layout.set_font_description(Some(&font));
    layout.set_text("Ag");
    layout.pixel_size().1.max(1)
}

fn cell_caret_pixel_x(entry: &gtk4::Entry, index: i32) -> i32 {
    let text = entry.text().to_string();
    let layout = pango::Layout::new(&entry.pango_context());
    if let Some(font) = entry.pango_context().font_description() {
        layout.set_font_description(Some(&font));
    }
    layout.set_text(&text);
    let (text_w, _) = layout.pixel_size();
    let pad = 8;
    let extra = {
        let inner = (entry.width() - 2 * pad - text_w).max(0) as f32;
        let align = gtk4::prelude::EditableExt::alignment(entry);
        (inner * align) as i32
    };
    let prefix: String = text.chars().take(index.max(0) as usize).collect();
    layout.set_text(&prefix);
    let (prefix_w, _) = layout.pixel_size();
    pad + extra + prefix_w
}

fn grid_columns(grid: &gtk4::Grid) -> usize {
    let mut cols = 0usize;
    while grid.child_at(cols as i32, 0).is_some() && cols < 64 {
        cols += 1;
    }
    cols
}

fn grid_rows(grid: &gtk4::Grid) -> usize {
    let mut rows = 0usize;
    while grid.child_at(0, rows as i32).is_some() && rows < 256 {
        rows += 1;
    }
    rows
}

fn text_selection(typing: bool, caret: i32, anchor: i32) -> Option<(i32, i32)> {
    if typing && caret != anchor {
        Some((caret.min(anchor), caret.max(anchor)))
    } else {
        None
    }
}

fn chars_of(entry: &gtk4::Entry) -> i32 {
    entry.text().chars().count() as i32
}

fn cell_index_at_x(entry: &gtk4::Entry, x: f64) -> i32 {
    let text = entry.text().to_string();
    if text.is_empty() {
        return 0;
    }
    let layout = pango::Layout::new(&entry.pango_context());
    layout.set_text(&text);
    if let Some(font) = entry.pango_context().font_description() {
        layout.set_font_description(Some(&font));
    }
    let pad = 8.0;
    let x_pango = ((x - pad).max(0.0) * f64::from(pango::SCALE)) as i32;
    let (_, byte_index, _) = layout.xy_to_index(x_pango, 0);
    let mut byte_index = (byte_index as usize).min(text.len());
    while byte_index > 0 && !text.is_char_boundary(byte_index) {
        byte_index -= 1;
    }
    text[..byte_index].chars().count() as i32
}

fn splice_chars(text: &str, start: i32, end: i32, insert: &str) -> (String, i32) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len() as i32;
    let mut start = start.clamp(0, n) as usize;
    let mut end = end.clamp(0, n) as usize;
    if start > end {
        std::mem::swap(&mut start, &mut end);
    }
    let mut out = String::new();
    out.extend(chars[..start].iter());
    out.push_str(insert);
    out.extend(chars[end..].iter());
    let pos = start as i32 + insert.chars().count() as i32;
    (out, pos)
}

fn skip_word(text: &str, pos: i32, dir: i32) -> i32 {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len() as i32;
    let mut i = pos.clamp(0, n);
    if dir < 0 {
        if i == 0 {
            return 0;
        }
        i -= 1;
        while i > 0 && chars[i as usize].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !chars[(i - 1) as usize].is_whitespace() {
            i -= 1;
        }
        i
    } else {
        while i < n && !chars[i as usize].is_whitespace() {
            i += 1;
        }
        while i < n && chars[i as usize].is_whitespace() {
            i += 1;
        }
        i
    }
}

thread_local! {
    /// Декодированные картинки по пути и времени изменения файла. Picture
    /// декодировал файл в главном потоке при каждой перестройке таблиц и
    /// картинок, при каждом возврате в файл и для каждой панели отдельно.
    static TEXTURES: RefCell<Vec<(PathBuf, std::time::SystemTime, gtk4::gdk::Texture)>> =
        const { RefCell::new(Vec::new()) };
}

/// Предел кэша по памяти: несжатая картинка 3000×2000 — это 24 МБ.
const TEXTURE_CACHE_BYTES: usize = 96 * 1024 * 1024;

fn texture_bytes(texture: &gtk4::gdk::Texture) -> usize {
    texture.width().max(0) as usize * texture.height().max(0) as usize * 4
}

fn cached_texture(path: &Path) -> Option<gtk4::gdk::Texture> {
    let modified = std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()?;
    let hit = TEXTURES.with(|cache| {
        let mut cache = cache.borrow_mut();
        let at = cache
            .iter()
            .position(|(p, m, _)| p == path && *m == modified)?;
        let entry = cache.remove(at);
        let texture = entry.2.clone();
        cache.push(entry);
        Some(texture)
    });
    if hit.is_some() {
        return hit;
    }
    let texture = gtk4::gdk::Texture::from_file(&gtk4::gio::File::for_path(path)).ok()?;
    TEXTURES.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.retain(|(p, _, _)| p != path);
        cache.push((path.to_path_buf(), modified, texture.clone()));
        // Самые давние вон, пока не уложимся; последняя остаётся всегда.
        while cache.len() > 1
            && cache
                .iter()
                .map(|(_, _, t)| texture_bytes(t))
                .sum::<usize>()
                > TEXTURE_CACHE_BYTES
        {
            cache.remove(0);
        }
    });
    Some(texture)
}

fn image_widget(path: Option<&Path>) -> gtk4::Widget {
    if let Some(texture) = path.filter(|path| path.is_file()).and_then(cached_texture) {
        let picture = gtk4::Picture::for_paintable(&texture);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk4::ContentFit::ScaleDown);
        picture.set_size_request(-1, 120);
        picture.upcast()
    } else {
        let label = gtk4::Label::new(Some("[изображение]"));
        label.add_css_class("dim-label");
        label.upcast()
    }
}

fn resolve_image(from: &Path, dest: &str) -> Option<PathBuf> {
    if dest.starts_with("http://") || dest.starts_with("https://") {
        return None;
    }
    let parent = from.parent().unwrap_or_else(|| Path::new("."));
    let path = parent.join(dest);
    path.canonicalize().ok().or(Some(path))
}

#[cfg(test)]
mod tests {
    use super::changed_span;

    fn apply(old: &str, new: &str) -> String {
        let (from, old_to, new_to) = changed_span(old, new);
        format!("{}{}{}", &old[..from], &new[from..new_to], &old[old_to..])
    }

    #[test]
    fn changed_lines_finds_pasted_and_restyled_lines() {
        use super::{changed_lines, line_signatures};
        use text_workspace::markup::mark_spans;
        let sig = |t: &str| line_signatures(t, &mark_spans(t));
        let before = "абзац\n\nвторой\nтретий\n";
        let pasted = "абзац\n# Заголовок\n**жирный**\n\nвторой\nтретий\n";
        assert_eq!(changed_lines(&sig(before), &sig(pasted)), vec![1, 2]);
        // Открытая ``` меняет оформление следующих строк при том же тексте.
        let fenced = "```\nабзац\n\nвторой\nтретий\n";
        let lines = changed_lines(&sig(before), &sig(fenced));
        assert!(
            lines.contains(&0) && lines.contains(&1) && lines.contains(&3),
            "{lines:?}"
        );
        assert!(changed_lines(&sig(before), &sig(before)).is_empty());
    }

    #[test]
    fn changed_span_is_minimal_and_char_safe() {
        assert_eq!(changed_span("abc", "abc"), (3, 3, 3));
        assert_eq!(changed_span("| а | б |\n", "| а | в | б |\n"), (7, 7, 12));
        for (old, new) in [
            ("ёж", "ём"),
            ("жёлтый", "жёлтый дом"),
            ("ааа", "а"),
            ("", "текст"),
            ("текст", ""),
            ("строка ж", "строка ё"),
        ] {
            assert_eq!(apply(old, new), new, "{old} -> {new}");
        }
    }
}
