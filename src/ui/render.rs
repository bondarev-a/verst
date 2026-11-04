use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4::gdk;
use gtk4::glib::object::IsA;
use gtk4::prelude::*;
use gtk4::{glib, pango};
use text_workspace::markup::{mark_spans, MarkKind, MarkSpan};
use text_workspace::session::ViewMode;
use text_workspace::tables::{self, ColAlign, MdTable};
use text_workspace::markdown;

use super::support::{buffer_text_of, is_markdown};
use super::{Ctx, PREVIEW_MS};

const TAG_HEADING: &[&str] = &[
    "tw-h1", "tw-h2", "tw-h3", "tw-h4", "tw-h5", "tw-h6",
];
const TAG_STRONG: &str = "tw-strong";
const TAG_EM: &str = "tw-em";
const TAG_CODE: &str = "tw-code";
const TAG_CODEBLOCK: &str = "tw-codeblock";
const TAG_HIDE: &str = "tw-hide";
const TAG_TABLE: &str = "tw-table";
const TAG_LINK: &str = "tw-link";
const TAG_MONO: &str = "tw-mono";

#[derive(Default)]
pub(super) struct RenderState {
    pub writing: bool,
    tick: u64,
    tables: Vec<LiveTable>,
    images: Vec<LiveImage>,
    cursor_hooked: Vec<*const sourceview5::Buffer>,
    /// Разбор текущего текста. Курсор его не пересчитывает.
    spans: Vec<MarkSpan>,
    line_starts: Vec<usize>,
    text_len: usize,
    cursor_line: Option<i32>,
    painted: bool,
    table_bodies: Vec<String>,
    image_keys: Vec<String>,
}

struct LiveTable {
    index: usize,
    start: usize,
    end: usize,
    start_line: i32,
    end_line: i32,
    anchor: gtk4::TextChildAnchor,
    grids: [gtk4::Grid; 2],
    focus_cell: Rc<RefCell<Option<(usize, usize)>>>,
}

struct LiveImage {
    anchor: gtk4::TextChildAnchor,
    widgets: [gtk4::Widget; 2],
}

impl Ctx {
    pub(super) fn install_render_clicks(&self) {
        for view in &self.widgets.views {
            let click = gtk4::GestureClick::new();
            click.set_button(1);
            let ctx = self.clone();
            let view_click = view.clone();
            click.connect_pressed(move |gesture, _, x, y| {
                if ctx.state.borrow().view_mode != ViewMode::Render {
                    return;
                }
                let Some(event) = gesture.current_event() else {
                    return;
                };
                if !event.modifier_state().contains(gdk::ModifierType::CONTROL_MASK) {
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
                    if let MarkKind::Link { dest } = &span.kind {
                        if offset >= span.start && offset < span.end {
                            if let Some(target) = markdown::resolve_markdown(&path, dest) {
                                ctx.go(target, None);
                            }
                            gesture.set_state(gtk4::EventSequenceState::Claimed);
                            return;
                        }
                    }
                }
            });
            view.add_controller(click);
        }
    }

    pub(super) fn schedule_render(&self) {
        if self.state.borrow().view_mode != ViewMode::Render {
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
            if ctx.state.borrow().view_mode != ViewMode::Render {
                return;
            }
            ctx.refresh_render_now();
        });
    }

    pub(super) fn refresh_render_now(&self) {
        if self.state.borrow().view_mode != ViewMode::Render {
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
        self.ensure_cursor_hook(&buffer);
        let text = buffer_text_of(&buffer);
        let cursor_line = buffer.iter_at_mark(&buffer.get_insert()).line();
        let cell_focus = self.any_table_cell_focused();
        if cell_focus {
            let spans = mark_spans(&text);
            paint_tags(&buffer, &text, &spans, cursor_line);
            let mut state = self.state.borrow_mut();
            state.render.line_starts = line_starts_of(&text);
            state.render.text_len = text.len();
            state.render.spans = spans;
            state.render.cursor_line = Some(cursor_line);
            return;
        }
        self.rebuild_widgets(&buffer, &path, cursor_line);
    }

    pub(super) fn clear_render_layer(&self) {
        let (tables, images) = {
            let mut state = self.state.borrow_mut();
            state.render.spans.clear();
            state.render.line_starts.clear();
            state.render.table_bodies.clear();
            state.render.image_keys.clear();
            state.render.painted = false;
            state.render.cursor_line = None;
            (
                std::mem::take(&mut state.render.tables),
                std::mem::take(&mut state.render.images),
            )
        };
        for table in &tables {
            for grid in &table.grids {
                detach_child(grid);
            }
        }
        for image in &images {
            for widget in &image.widgets {
                detach_child(widget);
            }
        }
        let buffers: Vec<sourceview5::Buffer> = self
            .state
            .borrow()
            .buffers
            .values()
            .map(|entry| entry.buffer.clone())
            .collect();
        for buffer in &buffers {
            self.state.borrow_mut().render.writing = true;
            buffer.begin_irreversible_action();
            remove_anchors(
                &buffer.clone().upcast::<gtk4::TextBuffer>(),
                &tables,
                &images,
            );
            buffer.end_irreversible_action();
            self.state.borrow_mut().render.writing = false;
            clear_render_tags(buffer);
        }
    }

    pub(super) fn apply_render_mode_chrome(&self) {
        let mode = self.state.borrow().view_mode;
        let path = self.focused_path();
        let markdown = path.as_ref().is_some_and(|path| is_markdown(path));
        for view in &self.widgets.views {
            view.set_monospace(mode != ViewMode::Render || !markdown);
        }
        if mode == ViewMode::Render && markdown {
            self.schedule_render();
        } else {
            self.clear_render_layer();
        }
    }

    pub(super) fn add_table_column(&self) {
        if self.state.borrow().view_mode != ViewMode::Render {
            self.set_status("Колонка таблицы добавляется в режиме рендеринга");
            return;
        }
        let Some(path) = self.focused_path() else {
            return;
        };
        if !is_markdown(&path) {
            return;
        }
        let index = self.focused_table_index().or_else(|| {
            self.state
                .borrow()
                .render
                .tables
                .first()
                .map(|table| table.index)
        });
        let Some(index) = index else {
            self.set_status("Нет таблицы под курсором");
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
        let Some(next) = tables::add_column(&text, index) else {
            return;
        };
        self.replace_all_text(&buffer, &next);
        self.refresh_render_now();
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
        // Синхронизировать текст ячеек во втором поле.
        self.sync_table_cell_entries(table, row, col, value.trim());
    }

    pub(super) fn table_add_row(&self, path: PathBuf, table: usize) {
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
        let Some(next) = tables::add_row(&text, table) else {
            return;
        };
        self.replace_all_text(&buffer, &next);
        self.refresh_render_now();
    }

    fn sync_table_cell_entries(&self, table: usize, row: usize, col: usize, value: &str) {
        let state = self.state.borrow();
        let Some(live) = state.render.tables.iter().find(|item| item.index == table) else {
            return;
        };
        for grid in &live.grids {
            if let Some(entry) = grid
                .child_at(col as i32, row as i32)
                .and_then(|widget| widget.downcast::<gtk4::Entry>().ok())
            {
                if entry.text() != value {
                    entry.set_text(value);
                }
            }
        }
    }

    fn replace_all_text(&self, buffer: &sourceview5::Buffer, next: &str) {
        self.state.borrow_mut().render.writing = true;
        buffer.begin_user_action();
        buffer.set_text(next);
        buffer.end_user_action();
        self.state.borrow_mut().render.writing = false;
    }

    fn any_table_cell_focused(&self) -> bool {
        let state = self.state.borrow();
        state.render.tables.iter().any(|table| {
            table.focus_cell.borrow().is_some() || table.grids.iter().any(grid_has_focus)
        })
    }

    fn focused_table_index(&self) -> Option<usize> {
        let state = self.state.borrow();
        state.render.tables.iter().find_map(|table| {
            if table.focus_cell.borrow().is_some() || table.grids.iter().any(grid_has_focus) {
                Some(table.index)
            } else {
                None
            }
        })
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
        ensure_tag(&table, TAG_TABLE, hide_source);
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
    }

    fn ensure_cursor_hook(&self, buffer: &sourceview5::Buffer) {
        let ptr = buffer as *const sourceview5::Buffer;
        if self
            .state
            .borrow()
            .render
            .cursor_hooked
            .iter()
            .any(|item| *item == ptr)
        {
            return;
        }
        self.state.borrow_mut().render.cursor_hooked.push(ptr);
        let ctx = self.clone();
        buffer.connect_mark_set(move |_buf, iter, mark| {
            if mark.name().as_deref() != Some("insert") {
                return;
            }
            if ctx.state.borrow().view_mode != ViewMode::Render {
                return;
            }
            if ctx.state.borrow().render.writing {
                return;
            }
            let line = iter.line();
            if ctx.state.borrow().render.cursor_line == Some(line) {
                return;
            }
            if !ctx.state.borrow().render.painted {
                return;
            }
            let hit = ctx
                .state
                .borrow()
                .render
                .tables
                .iter()
                .find(|table| line >= table.start_line && line < table.end_line)
                .map(|table| table.index);
            if let Some(index) = hit {
                ctx.state.borrow_mut().render.cursor_line = Some(line);
                ctx.focus_table_cell(index, 0, 0);
                return;
            }
            ctx.shift_cursor_line(line);
        });
    }

    fn focus_table_cell(&self, table: usize, row: usize, col: usize) {
        let state = self.state.borrow();
        let Some(live) = state.render.tables.iter().find(|item| item.index == table) else {
            return;
        };
        let pane = self.focused_index();
        if let Some(child) = live.grids[pane]
            .child_at(col as i32, row as i32)
            .and_then(|widget| widget.downcast::<gtk4::Entry>().ok())
        {
            child.grab_focus();
        }
    }

    fn shift_cursor_line(&self, line: i32) {
        let prev = self.state.borrow().render.cursor_line;
        if prev == Some(line) {
            return;
        }
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
        let spans = std::mem::take(&mut self.state.borrow_mut().render.spans);
        let line_starts = self.state.borrow().render.line_starts.clone();
        let text_len = self.state.borrow().render.text_len;
        self.state.borrow_mut().render.cursor_line = Some(line);
        if let Some(prev) = prev {
            retag_cursor_line(&buffer, &spans, &line_starts, text_len, prev, false);
        }
        retag_cursor_line(&buffer, &spans, &line_starts, text_len, line, true);
        self.state.borrow_mut().render.spans = spans;
    }

    fn rebuild_widgets(
        &self,
        buffer: &sourceview5::Buffer,
        path: &Path,
        cursor_line: i32,
    ) {
        let text = buffer_text_of(buffer);
        let spans = mark_spans(&text);
        let md_tables = tables::parse_tables(&text);
        let bodies: Vec<String> = md_tables
            .iter()
            .map(|table| text[table.start.min(text.len())..table.end.min(text.len())].to_string())
            .collect();
        let image_keys: Vec<String> = spans
            .iter()
            .filter_map(|span| match &span.kind {
                MarkKind::Image { dest } => Some(format!(
                    "{dest}\n{}",
                    &text[span.start.min(text.len())..span.end.min(text.len())]
                )),
                _ => None,
            })
            .collect();
        let same_widgets = {
            let state = self.state.borrow();
            state.render.painted
                && state.render.table_bodies == bodies
                && state.render.image_keys == image_keys
        };
        if same_widgets {
            paint_tags(buffer, &text, &spans, cursor_line);
            cover_embedded_source(buffer, &text, &md_tables, &spans, cursor_line);
            {
                let mut state = self.state.borrow_mut();
                if state.render.tables.len() == md_tables.len() {
                    for (live, table) in state.render.tables.iter_mut().zip(md_tables.iter()) {
                        live.start = table.start;
                        live.end = table.end;
                        live.start_line = newline_count(&text[..table.start.min(text.len())]);
                        let mut end_line = newline_count(&text[..table.end.min(text.len())]);
                        if end_line <= live.start_line {
                            end_line = live.start_line + 1;
                        }
                        live.end_line = end_line;
                    }
                }
                lift_embedded(
                    buffer,
                    &state.render.tables,
                    &state.render.images,
                );
            }
            self.store_paint(&text, spans, cursor_line, bodies, image_keys);
            return;
        }

        let (old_tables, old_images) = {
            let mut state = self.state.borrow_mut();
            (
                std::mem::take(&mut state.render.tables),
                std::mem::take(&mut state.render.images),
            )
        };
        for table in &old_tables {
            for grid in &table.grids {
                detach_child(grid);
            }
        }
        for image in &old_images {
            for widget in &image.widgets {
                detach_child(widget);
            }
        }

        self.state.borrow_mut().render.writing = true;
        buffer.begin_irreversible_action();
        remove_anchors(
            &buffer.clone().upcast::<gtk4::TextBuffer>(),
            &old_tables,
            &old_images,
        );
        buffer.end_irreversible_action();
        self.state.borrow_mut().render.writing = false;

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
                        end: span.end,
                        dest: dest.clone(),
                    });
                }
            }
        }
        placements.sort_by_key(|item| std::cmp::Reverse(item.start()));

        let mut live_tables = Vec::new();
        let mut live_images = Vec::new();

        self.state.borrow_mut().render.writing = true;
        buffer.begin_irreversible_action();
        for place in placements {
            match place {
                Placement::Table { index, .. } => {
                    let Some(table) = md_tables.get(index) else {
                        continue;
                    };
                    if let Some(live) = self.mount_table(buffer, path, &text, table, index) {
                        live_tables.push(live);
                    }
                }
                Placement::Image { start, dest, .. } => {
                    if let Some(image) = self.mount_image(buffer, path, start, &dest) {
                        live_images.push(image);
                    }
                }
            }
        }
        buffer.end_irreversible_action();
        self.state.borrow_mut().render.writing = false;
        cover_embedded_source(buffer, &text, &md_tables, &spans, cursor_line);
        lift_embedded(buffer, &live_tables, &live_images);

        live_tables.sort_by_key(|table| table.start);
        {
            let mut state = self.state.borrow_mut();
            state.render.tables = live_tables;
            state.render.images = live_images;
        }
        self.store_paint(&text, spans, cursor_line, bodies, image_keys);
    }

    fn store_paint(
        &self,
        text: &str,
        spans: Vec<MarkSpan>,
        cursor_line: i32,
        bodies: Vec<String>,
        image_keys: Vec<String>,
    ) {
        let mut state = self.state.borrow_mut();
        state.render.line_starts = line_starts_of(text);
        state.render.text_len = text.len();
        state.render.spans = spans;
        state.render.cursor_line = Some(cursor_line);
        state.render.table_bodies = bodies;
        state.render.image_keys = image_keys;
        state.render.painted = true;
    }

    fn mount_table(
        &self,
        buffer: &sourceview5::Buffer,
        path: &Path,
        _text: &str,
        table: &MdTable,
        index: usize,
    ) -> Option<LiveTable> {
        let text = buffer_text_of(buffer);
        let mut iter = iter_at_file_byte(buffer, &text, table.start);
        let anchor = buffer.create_child_anchor(&mut iter);
        lift_anchor_scale(buffer, &anchor);
        let focus_cell = Rc::new(RefCell::new(None));
        let grids = [
            build_table_grid(self, path, table, index, focus_cell.clone()),
            build_table_grid(self, path, table, index, focus_cell.clone()),
        ];
        for (pane, grid) in grids.iter().enumerate() {
            self.widgets.views[pane].add_child_at_anchor(grid, &anchor);
            let width = self.widgets.views[pane].width().max(120);
            grid.set_size_request(width.saturating_sub(32), -1);
        }
        let start_line = newline_count(&text[..table.start.min(text.len())]);
        let mut end_line = newline_count(&text[..table.end.min(text.len())]);
        if end_line <= start_line {
            end_line = start_line + 1;
        }
        Some(LiveTable {
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
        start: usize,
        dest: &str,
    ) -> Option<LiveImage> {
        let text = buffer_text_of(buffer);
        let mut iter = iter_at_file_byte(buffer, &text, start);
        let anchor = buffer.create_child_anchor(&mut iter);
        lift_anchor_scale(buffer, &anchor);
        let file = resolve_image(path, dest);
        let w0 = image_widget(file.as_deref());
        let w1 = image_widget(file.as_deref());
        self.widgets.views[0].add_child_at_anchor(&w0, &anchor);
        self.widgets.views[1].add_child_at_anchor(&w1, &anchor);
        Some(LiveImage {
            anchor,
            widgets: [w0, w1],
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
    Image { start: usize, end: usize, dest: String },
}

impl Placement {
    fn start(&self) -> usize {
        match self {
            Self::Table { start, .. } | Self::Image { start, .. } => *start,
        }
    }
}

fn build_table_grid(
    ctx: &Ctx,
    path: &Path,
    table: &MdTable,
    table_index: usize,
    focus_cell: Rc<RefCell<Option<(usize, usize)>>>,
) -> gtk4::Grid {
    let grid = gtk4::Grid::new();
    grid.add_css_class("tw-md-table");
    grid.set_column_homogeneous(true);
    grid.set_column_spacing(0);
    grid.set_row_spacing(0);
    grid.set_hexpand(true);

    let rows = table.row_count();
    let cols = table.column_count();
    for row in 0..rows {
        for col in 0..cols {
            let raw = table
                .cell(row, col)
                .map(|cell| cell.text.clone())
                .unwrap_or_default();
            let entry = gtk4::Entry::new();
            entry.set_text(raw.trim());
            entry.set_hexpand(true);
            entry.add_css_class("tw-md-cell");
            if row == 0 {
                entry.add_css_class("tw-md-cell-head");
            }
            match table.alignments.get(col).copied().unwrap_or(ColAlign::None) {
                ColAlign::Left | ColAlign::None => {
                    gtk4::prelude::EditableExt::set_alignment(&entry, 0.0);
                }
                ColAlign::Center => gtk4::prelude::EditableExt::set_alignment(&entry, 0.5),
                ColAlign::Right => gtk4::prelude::EditableExt::set_alignment(&entry, 1.0),
            }

            let focus = focus_cell.clone();
            entry.connect_has_focus_notify(move |entry| {
                if entry.has_focus() {
                    *focus.borrow_mut() = Some((row, col));
                } else if focus.borrow().as_ref() == Some(&(row, col)) {
                    *focus.borrow_mut() = None;
                }
            });

            let ctx_commit = ctx.clone();
            let path_commit = path.to_path_buf();
            let entry_commit = entry.clone();
            entry.connect_has_focus_notify(move |entry| {
                if entry.has_focus() {
                    return;
                }
                ctx_commit.commit_table_cell(
                    path_commit.clone(),
                    table_index,
                    row,
                    col,
                    format!(" {} ", entry_commit.text()),
                );
            });

            let key = gtk4::EventControllerKey::new();
            let ctx_key = ctx.clone();
            let path_key = path.to_path_buf();
            let grid_key = grid.clone();
            let focus_key = focus_cell.clone();
            key.connect_key_pressed(move |_, keyval, _, _| {
                if keyval == gdk::Key::Tab {
                    let (nr, nc) = if col + 1 < cols {
                        (row, col + 1)
                    } else if row + 1 < rows {
                        (row + 1, 0)
                    } else {
                        ctx_key.table_add_row(path_key.clone(), table_index);
                        return glib::Propagation::Stop;
                    };
                    if let Some(child) = grid_key
                        .child_at(nc as i32, nr as i32)
                        .and_then(|widget| widget.downcast::<gtk4::Entry>().ok())
                    {
                        child.grab_focus();
                    }
                    return glib::Propagation::Stop;
                }
                if keyval == gdk::Key::Return || keyval == gdk::Key::KP_Enter {
                    if row + 1 < rows {
                        if let Some(child) = grid_key
                            .child_at(col as i32, (row + 1) as i32)
                            .and_then(|widget| widget.downcast::<gtk4::Entry>().ok())
                        {
                            child.grab_focus();
                        }
                    } else {
                        ctx_key.table_add_row(path_key.clone(), table_index);
                    }
                    return glib::Propagation::Stop;
                }
                if keyval == gdk::Key::Up && row == 0 {
                    *focus_key.borrow_mut() = None;
                    ctx_key.place_cursor_near_table(table_index, true);
                    return glib::Propagation::Stop;
                }
                if keyval == gdk::Key::Down && row + 1 == rows {
                    *focus_key.borrow_mut() = None;
                    ctx_key.place_cursor_near_table(table_index, false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            entry.add_controller(key);
            grid.attach(&entry, col as i32, row as i32, 1, 1);
        }
    }
    grid
}

fn paint_tags(
    buffer: &sourceview5::Buffer,
    text: &str,
    spans: &[MarkSpan],
    cursor_line: i32,
) {
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
            MarkKind::Link { .. } => jobs.push((span.start, span.end, TAG_LINK)),
            MarkKind::Image { .. } => {}
        }
    }
    if let Some((start, end)) = cursor_range {
        jobs.push((start, end, TAG_MONO));
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
    let cursor_range = line_byte_range(text, cursor_line);
    let mut jobs: Vec<(usize, usize, &'static str)> = Vec::new();
    for table in tables {
        jobs.push((table.start, table.end, TAG_TABLE));
    }
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
    let mut walk = ByteCursor::new(buffer, text);
    for (start, end, name) in jobs {
        walk.apply(buffer, start, end, name);
    }
}

fn lift_anchor_scale(buffer: &sourceview5::Buffer, anchor: &gtk4::TextChildAnchor) {
    let a = buffer.iter_at_child_anchor(anchor);
    let mut b = a.clone();
    if !b.forward_char() {
        return;
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_TABLE) {
        buffer.remove_tag(&tag, &a, &b);
    }
    if let Some(tag) = buffer.tag_table().lookup(TAG_HIDE) {
        buffer.remove_tag(&tag, &a, &b);
    }
}

fn lift_embedded(buffer: &sourceview5::Buffer, tables: &[LiveTable], images: &[LiveImage]) {
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
) {
    if line < 0 {
        return;
    }
    let Some(&start) = line_starts.get(line as usize) else {
        return;
    };
    let end = line_starts
        .get(line as usize + 1)
        .copied()
        .unwrap_or(text_len);
    let Some(from) = buffer.iter_at_line(line) else {
        return;
    };
    let mut to = from.clone();
    if !to.forward_line() {
        to = buffer.end_iter();
    }
    let table = buffer.tag_table();
    if let Some(tag) = table.lookup(TAG_HIDE) {
        buffer.remove_tag(&tag, &from, &to);
    }
    if let Some(tag) = table.lookup(TAG_MONO) {
        buffer.remove_tag(&tag, &from, &to);
    }
    let line_text = buffer.text(&from, &to, false);
    if on_cursor {
        if let Some(tag) = table.lookup(TAG_MONO) {
            buffer.apply_tag(&tag, &from, &to);
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
            MarkKind::HeadingMark | MarkKind::Delimiter | MarkKind::LinkMarkup if !on_cursor => {
                pieces.push((piece_start, piece_end, TAG_HIDE));
            }
            MarkKind::Code | MarkKind::CodeBlock => {
                pieces.push((piece_start, piece_end, TAG_MONO));
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
}

fn forward_in_line(
    line_text: &str,
    line_start: usize,
    iter: &mut gtk4::TextIter,
    at: &mut usize,
    byte: usize,
) -> gtk4::TextIter {
    if byte > *at {
        let from = at.saturating_sub(line_start).min(line_text.len());
        let to = byte.saturating_sub(line_start).min(line_text.len());
        if from < to {
            let chars = line_text[from..to].chars().count();
            iter.forward_chars(chars as i32);
        }
        *at = byte;
    }
    iter.clone()
}

struct ByteCursor<'a> {
    buffer: &'a sourceview5::Buffer,
    text: &'a str,
    iter: gtk4::TextIter,
    byte: usize,
}

impl<'a> ByteCursor<'a> {
    fn new(buffer: &'a sourceview5::Buffer, text: &'a str) -> Self {
        Self {
            buffer,
            text,
            iter: buffer.start_iter(),
            byte: 0,
        }
    }

    fn skip_anchors(&mut self) {
        while !self.iter.is_end() && self.iter.child_anchor().is_some() {
            if !self.iter.forward_char() {
                break;
            }
        }
    }

    fn seek(&mut self, byte: usize) -> gtk4::TextIter {
        let byte = byte.min(self.text.len());
        if byte < self.byte {
            self.iter = self.buffer.start_iter();
            self.byte = 0;
        }
        self.skip_anchors();
        let extra = self.text[self.byte..byte].chars().count();
        let mut left = extra;
        while left > 0 && !self.iter.is_end() {
            if self.iter.child_anchor().is_some() {
                if !self.iter.forward_char() {
                    break;
                }
                continue;
            }
            if !self.iter.forward_char() {
                break;
            }
            left -= 1;
        }
        self.skip_anchors();
        self.byte = byte;
        self.iter.clone()
    }

    fn apply(&mut self, buffer: &sourceview5::Buffer, start: usize, end: usize, name: &str) {
        if start >= end || end > self.text.len() {
            return;
        }
        let Some(tag) = buffer.tag_table().lookup(name) else {
            return;
        };
        let a = self.seek(start);
        let b = self.seek(end);
        buffer.apply_tag(&tag, &a, &b);
    }
}

fn clear_render_tags(buffer: &sourceview5::Buffer) {
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

/// Служебный текст не помечается invisible: на длинном файле GTK при прокрутке
/// падает с «Byte index is off the end of the line».
fn hide_source(tag: &gtk4::TextTag) {
    tag.set_scale(0.001);
    tag.set_foreground_rgba(Some(&gtk4::gdk::RGBA::new(0.0, 0.0, 0.0, 0.0)));
}

fn ensure_tag(table: &gtk4::TextTagTable, name: &str, configure: impl FnOnce(&gtk4::TextTag)) {
    if table.lookup(name).is_some() {
        return;
    }
    let tag = gtk4::TextTag::new(Some(name));
    configure(&tag);
    table.add(&tag);
}

fn iter_at_file_byte(
    buffer: &sourceview5::Buffer,
    text: &str,
    byte: usize,
) -> gtk4::TextIter {
    let target_chars = text[..byte.min(text.len())].chars().count();
    let mut iter = buffer.start_iter();
    let mut seen = 0usize;
    loop {
        if iter.is_end() {
            break;
        }
        if iter.child_anchor().is_some() {
            if !iter.forward_char() {
                break;
            }
            continue;
        }
        if seen == target_chars {
            break;
        }
        seen += 1;
        if !iter.forward_char() {
            break;
        }
    }
    iter
}

fn file_byte_at_iter(buffer: &gtk4::TextBuffer, iter: &gtk4::TextIter) -> usize {
    let start = buffer.start_iter();
    buffer.text(&start, iter, true).len()
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

fn remove_anchors(buffer: &gtk4::TextBuffer, tables: &[LiveTable], images: &[LiveImage]) {
    let mut positions: Vec<(i32, gtk4::TextChildAnchor)> = Vec::new();
    for table in tables {
        if !table.anchor.is_deleted() {
            positions.push((
                buffer.iter_at_child_anchor(&table.anchor).offset(),
                table.anchor.clone(),
            ));
        }
    }
    for image in images {
        if !image.anchor.is_deleted() {
            positions.push((
                buffer.iter_at_child_anchor(&image.anchor).offset(),
                image.anchor.clone(),
            ));
        }
    }
    positions.sort_by_key(|(offset, _)| std::cmp::Reverse(*offset));
    for (_, anchor) in positions {
        if anchor.is_deleted() {
            continue;
        }
        let mut iter = buffer.iter_at_child_anchor(&anchor);
        let mut end = iter.clone();
        end.forward_char();
        buffer.delete(&mut iter, &mut end);
    }
}

fn detach_child(widget: &impl IsA<gtk4::Widget>) {
    if let Some(parent) = widget.parent() {
        if let Ok(view) = parent.downcast::<gtk4::TextView>() {
            view.remove(widget);
        }
    }
}

fn grid_has_focus(grid: &gtk4::Grid) -> bool {
    let mut child = grid.first_child();
    while let Some(widget) = child {
        if widget.has_focus() {
            return true;
        }
        child = widget.next_sibling();
    }
    false
}

fn image_widget(path: Option<&Path>) -> gtk4::Widget {
    if let Some(path) = path.filter(|path| path.is_file()) {
        let picture = gtk4::Picture::for_filename(path);
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
