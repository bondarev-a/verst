use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk4::prelude::*;
use gtk4::{gio, glib};
use webkit6::prelude::{PolicyDecisionExt, WebViewExt};

use text_workspace::session::ViewMode;
use text_workspace::{graph, index, kind, markdown, places, seams, session, zoom};

fn media_page(uri: &str, video: bool) -> String {
    let tag = if video { "video" } else { "audio" };
    let uri = uri.replace('&', "&amp;").replace('"', "&quot;");
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  body {{ margin: 0; height: 100vh; display: flex; align-items: center; justify-content: center; }}
  {tag} {{ max-width: 100%; max-height: 100%; }}
</style>
</head>
<body><{tag} src="{uri}" controls></{tag}></body>
</html>"#
    )
}

fn fill_outline(list: &gtk4::ListBox, headings: &[markdown::Heading]) {
    clear_list(list);
    if headings.is_empty() {
        let label = gtk4::Label::new(Some("Нет заголовков"));
        label.add_css_class("dim-label");
        label.set_halign(gtk4::Align::Start);
        label.set_margin_start(10);
        list.append(&label);
        return;
    }
    for heading in headings {
        let label = gtk4::Label::new(Some(&heading.text));
        label.set_halign(gtk4::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_margin_start(8 + i32::from(heading.level.saturating_sub(1)) * 14);
        label.set_tooltip_text(Some(&heading.text));
        list.append(&label);
    }
}

fn fact_row(title: &str, value: &str) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let name = gtk4::Label::new(Some(title));
    name.add_css_class("dim-label");
    name.set_halign(gtk4::Align::Start);
    name.set_width_chars(16);
    name.set_xalign(0.0);
    let shown = gtk4::Label::new(Some(value));
    shown.set_halign(gtk4::Align::Start);
    shown.set_hexpand(true);
    shown.set_selectable(true);
    shown.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    shown.set_tooltip_text(Some(value));
    row.append(&name);
    row.append(&shown);
    row
}

fn fill_path_rows(list: &gtk4::ListBox, files: &[PathBuf]) {
    for path in files {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        row.add_css_class("file-row");
        row.set_can_target(false);
        row.append(&file_icon(path));
        let label = gtk4::Label::new(Some(&file_label(path)));
        label.set_halign(gtk4::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        label.set_tooltip_text(Some(&path.display().to_string()));
        row.append(&label);
        list.append(&row);
    }
}

fn note_row(list: &gtk4::ListBox, text: &str) {
    let label = gtk4::Label::new(Some(text));
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(8);
    label.add_css_class("dim-label");
    list.append(&label);
}

mod files;
mod navigate;
mod notes;
mod settings;
mod support;
use support::*;

pub(super) const AUTOSAVE_MS: u64 = 800;
pub(super) const PREVIEW_MS: u64 = 150;
pub(super) const MAX_OPEN_BYTES: u64 = 8 * 1024 * 1024;
pub(super) const TREE_ATTRS: &str =
    "standard::name,standard::display-name,standard::file,standard::symbolic-icon,standard::type";
pub(super) const FOLDER_ATTRS: &str = "standard::name,standard::display-name,standard::file,standard::symbolic-icon,standard::type,standard::size,time::modified";
const APP_TITLE: &str = "Text Workspace";

struct BufferEntry {
    buffer: sourceview5::Buffer,
    dirty: bool,
    last_saved: String,
    acked_external: Option<String>,
    // Монитор хранится здесь: если его сбросить, наблюдение за файлом прекратится.
    #[allow(dead_code)]
    monitor: Option<gio::FileMonitor>,
}

struct State {
    session: session::Session,
    view_mode: ViewMode,
    buffers: HashMap<PathBuf, BufferEntry>,
    index: index::LinkIndex,
    index_gen: u64,
    /// Граф ссылок другого запиненного каталога, не активного пина.
    scope_graph: Option<(PathBuf, index::LinkIndex)>,
    scope_building: Option<PathBuf>,
    loading: bool,
    dock_adjust: bool,
    conflict_open: bool,
    autosave: HashMap<PathBuf, glib::SourceId>,
    graph: graph::GraphLayout,
    headings: Vec<markdown::Heading>,
    search_hits: Vec<index::SearchHit>,
    backlinks: Vec<PathBuf>,
    tag_query: Option<String>,
    nav: places::NavHistory,
    /// Каталог, выбранный щелчком в дереве. Пока пусто — каталог открытого файла.
    nav_dir: Option<PathBuf>,
    browsing: bool,
    mode_tick: u64,
}

#[derive(Clone)]
struct Widgets {
    window: gtk4::ApplicationWindow,
    pins_box: gtk4::Box,
    open_mode_dropdown: gtk4::DropDown,
    workspace_slot: gtk4::Box,
    open_list: gtk4::ListBox,
    pinned_list: gtk4::ListBox,
    history_list: gtk4::ListBox,
    notes_host: gtk4::Box,
    notes_tags: gtk4::FlowBox,
    notes_tags_fold: gtk4::Expander,
    notes_tags_scroll: gtk4::ScrolledWindow,
    notes_tags_all: gtk4::Button,
    notes_new: gtk4::Button,
    notes_menu: gtk4::MenuButton,
    notes_group_flat: gtk4::Button,
    notes_group_created: gtk4::Button,
    notes_group_modified: gtk4::Button,
    notes_group_tags: gtk4::Button,
    main_paned: gtk4::Paned,
    right_paned: gtk4::Paned,
    nav_pin: gtk4::ToggleButton,
    nav_fold: gtk4::Button,
    activity_bar: gtk4::Box,
    activity_buttons: Vec<gtk4::Button>,
    nav_expand: gtk4::Button,
    nav_rail: gtk4::Box,
    nav_rail_pin: gtk4::ToggleButton,
    info_pin: gtk4::ToggleButton,
    info_fold: gtk4::Button,
    info_expand: gtk4::Button,
    info_rail: gtk4::Box,
    info_rail_pin: gtk4::ToggleButton,
    doc_side: gtk4::Box,
    views: [sourceview5::View; 2],
    second_scroll: gtk4::ScrolledWindow,
    preview: webkit6::WebView,
    picture: gtk4::Picture,
    outline_list: gtk4::ListBox,
    file_facts: gtk4::Box,
    search_entry: gtk4::SearchEntry,
    search_list: gtk4::ListBox,
    backlink_list: gtk4::ListBox,
    incoming_list: gtk4::Box,
    outgoing_list: gtk4::Box,
    incoming_fold: gtk4::Expander,
    outgoing_fold: gtk4::Expander,
    link_head: gtk4::Box,
    link_caption: gtk4::Label,
    link_clear: gtk4::Button,
    tag_box: gtk4::FlowBox,
    graph: gtk4::DrawingArea,
    left_notebook: gtk4::Notebook,
    right_notebook: gtk4::Notebook,
    menubar: gtk4::PopoverMenuBar,
    editor_paned: gtk4::Paned,
    disk_list: Rc<RefCell<Option<gtk4::ListView>>>,
    workspace_list: Rc<RefCell<Option<gtk4::ListView>>>,
    path_label: gtk4::Label,
    nav_path: gtk4::Label,
    back_button: gtk4::Button,
    forward_button: gtk4::Button,
    doc_label: gtk4::Label,
    catalog_button: gtk4::Button,
    pin_file_button: gtk4::Button,
    find_bar: gtk4::Box,
    find_entry: gtk4::SearchEntry,
    content_stack: gtk4::Stack,
    folder_dir: gtk4::DirectoryList,
    folder_view: gtk4::ColumnView,
    folder_crumbs: gtk4::Box,
    folder_up: gtk4::Button,
    mode_button: gtk4::Button,
    status: gtk4::Label,
    css: gtk4::CssProvider,
    empty_buffer: sourceview5::Buffer,
    preferences: Rc<RefCell<Option<gtk4::Window>>>,
    zoom_spin: Rc<RefCell<Option<gtk4::SpinButton>>>,
    open_mode_settings: Rc<RefCell<Option<gtk4::DropDown>>>,
    word_wrap_switch: Rc<RefCell<Option<gtk4::Switch>>>,
}

#[derive(Clone)]
struct Ctx {
    state: Rc<RefCell<State>>,
    widgets: Widgets,
}

thread_local! {
    static APP_CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn ensure_ctx(app: &gtk4::Application) -> Ctx {
    if let Some(ctx) = APP_CTX.with(|slot| slot.borrow().clone()) {
        return ctx;
    }
    let ctx = Ctx::new(app);
    ctx.restore();
    APP_CTX.with(|slot| *slot.borrow_mut() = Some(ctx.clone()));
    ctx
}

pub fn activate(app: &gtk4::Application) {
    let ctx = ensure_ctx(app);
    ctx.widgets.window.unminimize();
    ctx.widgets.window.present();
}

/// Сессия хранит размер с большого монитора. Окно больше самого маленького
/// экрана композитор кладёт туда целиком, и на ноутбуке его не видно.
fn fitted_size(width: i32, height: i32) -> (i32, i32) {
    let Some(display) = gtk4::gdk::Display::default() else {
        return (width, height);
    };
    let monitors = display.monitors();
    let mut limit_w = i32::MAX;
    let mut limit_h = i32::MAX;
    for index in 0..monitors.n_items() {
        let Some(monitor) = monitors
            .item(index)
            .and_then(|item| item.downcast::<gtk4::gdk::Monitor>().ok())
        else {
            continue;
        };
        let geo = monitor.geometry();
        if geo.width() > 0 && geo.height() > 0 {
            limit_w = limit_w.min(geo.width());
            limit_h = limit_h.min(geo.height());
        }
    }
    if limit_w == i32::MAX {
        return (width, height);
    }
    let limit_w = (limit_w - 80).max(session::MIN_WINDOW_WIDTH);
    let limit_h = (limit_h - 120).max(session::MIN_WINDOW_HEIGHT);
    (
        width.clamp(session::MIN_WINDOW_WIDTH, limit_w),
        height.clamp(session::MIN_WINDOW_HEIGHT, limit_h),
    )
}

pub fn open_files(app: &gtk4::Application, files: &[gio::File], _hint: &str) {
    let ctx = ensure_ctx(app);
    ctx.widgets.window.unminimize();
    ctx.widgets.window.present();
    let mut opened = false;
    for file in files {
        let Some(path) = file.path() else {
            continue;
        };
        if !path.is_file() {
            continue;
        }
        ctx.go(path, None);
        opened = true;
    }
    if !opened {
        ctx.set_status("Не удалось открыть файл");
    }
}

impl Ctx {
    fn new(app: &gtk4::Application) -> Self {
        let session = session::Session::load();
        let empty_buffer = sourceview5::Buffer::new(None);
        empty_buffer.set_enable_undo(true);
        let views = [make_view(&empty_buffer), make_view(&empty_buffer)];
        let workspace_slot = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let open_list = gtk4::ListBox::new();
        let pinned_list = gtk4::ListBox::new();
        let history_list = gtk4::ListBox::new();
        open_list.add_css_class("navigation-sidebar");
        pinned_list.add_css_class("navigation-sidebar");
        history_list.add_css_class("navigation-sidebar");
        open_list.add_css_class("file-list");
        pinned_list.add_css_class("file-list");
        history_list.add_css_class("file-list");
        let open_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        open_page.append(&side_caption("Закреплённые"));
        open_page.append(&pinned_list);
        open_page.append(&side_caption("Сейчас открыты"));
        open_page.append(&open_list);
        let disk_placeholder = gtk4::Label::new(Some("Проводник"));
        let notes_new = gtk4::Button::with_label("Новая заметка");
        notes_new.add_css_class("suggested-action");
        notes_new.set_hexpand(true);
        notes_new.set_tooltip_text(Some(
            "Создать заметку в личном каталоге. Теги пишутся в тексте как #тема",
        ));
        let notes_group_flat = gtk4::Button::with_label("Без группировки");
        let notes_group_created = gtk4::Button::with_label("По дате создания");
        let notes_group_modified = gtk4::Button::with_label("По дате изменения");
        let notes_group_tags = gtk4::Button::with_label("По тегам");
        for button in [
            &notes_group_flat,
            &notes_group_created,
            &notes_group_modified,
            &notes_group_tags,
        ] {
            button.set_halign(gtk4::Align::Fill);
        }
        let group_box = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
        group_box.set_margin_top(6);
        group_box.set_margin_bottom(6);
        group_box.set_margin_start(6);
        group_box.set_margin_end(6);
        group_box.append(&notes_group_flat);
        group_box.append(&notes_group_created);
        group_box.append(&notes_group_modified);
        group_box.append(&notes_group_tags);
        let group_popover = gtk4::Popover::new();
        group_popover.add_css_class("verst-popover");
        group_popover.set_child(Some(&group_box));
        let notes_menu = gtk4::MenuButton::new();
        notes_menu.set_icon_name("view-list-symbolic");
        notes_menu.set_label("Группировка");
        notes_menu.add_css_class("flat");
        notes_menu.set_tooltip_text(Some("Группировка заметок"));
        notes_menu.set_popover(Some(&group_popover));
        let notes_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        notes_bar.add_css_class("notes-bar");
        notes_bar.append(&notes_new);
        notes_bar.append(&notes_menu);
        let notes_hint = gtk4::Label::new(Some("Личные, пока не перенесены в пространство"));
        notes_hint.add_css_class("dim-label");
        notes_hint.add_css_class("side-caption");
        notes_hint.set_halign(gtk4::Align::Start);
        notes_hint.set_wrap(true);
        let notes_tags = gtk4::FlowBox::new();
        notes_tags.set_selection_mode(gtk4::SelectionMode::None);
        notes_tags.set_homogeneous(false);
        notes_tags.set_column_spacing(4);
        notes_tags.set_row_spacing(4);
        notes_tags.set_margin_start(8);
        notes_tags.set_margin_end(8);
        notes_tags.set_margin_top(4);
        notes_tags.set_margin_bottom(4);
        let notes_tags_scroll = gtk4::ScrolledWindow::new();
        notes_tags_scroll.set_child(Some(&notes_tags));
        notes_tags_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        notes_tags_scroll.set_propagate_natural_height(true);
        notes_tags_scroll.set_max_content_height(3 * 36);
        notes_tags_scroll.set_hexpand(true);
        let notes_tags_all = gtk4::Button::with_label("Все теги");
        notes_tags_all.add_css_class("flat");
        notes_tags_all.set_halign(gtk4::Align::Start);
        notes_tags_all.set_margin_start(8);
        notes_tags_all.set_margin_bottom(4);
        notes_tags_all.set_visible(false);
        notes_tags_all.set_tooltip_text(Some("Показать все теги, не больше пяти строк"));
        let notes_tags_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        notes_tags_box.append(&notes_tags_scroll);
        notes_tags_box.append(&notes_tags_all);
        let notes_tags_fold = gtk4::Expander::new(Some("Теги"));
        notes_tags_fold.set_expanded(session.note_tags_open);
        notes_tags_fold.set_hexpand(true);
        notes_tags_fold.set_margin_start(8);
        notes_tags_fold.set_margin_end(8);
        notes_tags_fold.set_margin_top(4);
        notes_tags_fold.set_visible(false);
        notes_tags_fold.set_child(Some(&notes_tags_box));
        let notes_host = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        notes_host.set_margin_top(4);
        let notes_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        notes_page.add_css_class("notes-page");
        notes_page.append(&notes_bar);
        notes_page.append(&notes_hint);
        notes_page.append(&notes_tags_fold);
        notes_page.append(&scrolled(&notes_host));
        let left = notebook_for(
            seams::PanelSide::Left,
            &[
                ("disk", disk_placeholder.upcast()),
                ("workspace", workspace_slot.clone().upcast()),
                ("notes", notes_page.upcast()),
                ("open", scrolled(&open_page).upcast()),
                ("history", scrolled(&history_list).upcast()),
            ],
        );
        left.set_width_request(session::NAV_MIN_WIDTH);
        left.add_css_class("sidebar-notebook");
        left.set_show_tabs(false);
        let nav_pin = dock_pin("Закрепить навигацию. Без булавки она схлопывается сама");
        let nav_fold = dock_icon("pan-start-symbolic", "Свернуть навигацию");
        nav_pin.add_css_class("activity-tool");
        nav_fold.add_css_class("activity-tool");
        nav_pin.set_focus_on_click(false);
        nav_fold.set_focus_on_click(false);
        if let Some(image) = nav_pin.child().and_downcast::<gtk4::Image>() {
            image.set_pixel_size(18);
            image.set_can_target(false);
        }
        if let Some(image) = nav_fold.child().and_downcast::<gtk4::Image>() {
            image.set_pixel_size(18);
            image.set_can_target(false);
        }
        let activity_icons = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        let mut activity_buttons = Vec::new();
        for id in &session.left_order {
            let Some(spec) = seams::panels().iter().find(|panel| {
                panel.side == seams::PanelSide::Left && panel.id == id.as_str()
            }) else {
                continue;
            };
            let button = activity_icon(spec.id, seams::panel_hint(spec.id), seams::panel_icon(spec.id));
            activity_icons.append(&button);
            activity_buttons.push(button);
        }
        let activity_spacer = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        activity_spacer.set_vexpand(true);
        let activity_bar = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        activity_bar.add_css_class("activity-bar");
        activity_bar.set_width_request(session::ACTIVITY_WIDTH);
        activity_bar.append(&activity_icons);
        activity_bar.append(&activity_spacer);
        activity_bar.append(&nav_pin);
        activity_bar.append(&nav_fold);
        let (nav_rail, nav_expand, nav_rail_pin) = dock_rail(
            "pan-end-symbolic",
            "Показать навигацию",
            "Закрепить навигацию. Без булавки она схлопывается сама",
        );
        let left_shell = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        left.set_hexpand(true);
        left.set_vexpand(true);
        left_shell.set_overflow(gtk4::Overflow::Hidden);
        let nav_path = gtk4::Label::new(None);
        nav_path.add_css_class("path-label");
        nav_path.add_css_class("nav-path");
        nav_path.set_halign(gtk4::Align::Start);
        nav_path.set_xalign(0.0);
        nav_path.set_hexpand(true);
        nav_path.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
        nav_path.set_visible(false);
        let nav_column = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        nav_column.set_hexpand(true);
        nav_column.set_vexpand(true);
        nav_column.append(&nav_path);
        nav_column.append(&left);
        left_shell.append(&activity_bar);
        left_shell.append(&nav_column);

        let scroll1 = scrolled(&views[0]);
        scroll1.set_width_request(420);
        let second_scroll = scrolled(&views[1]);
        second_scroll.set_visible(false);
        let editors = gtk4::Paned::new(gtk4::Orientation::Vertical);
        editors.set_start_child(Some(&caption("Исходник", &scroll1)));
        let second_caption = caption("Вторая панель", &second_scroll);
        second_caption.set_visible(false);
        editors.set_end_child(Some(&second_caption));
        editors.set_resize_start_child(true);
        editors.set_resize_end_child(true);
        editors.set_shrink_start_child(false);
        editors.set_shrink_end_child(true);
        editors.set_position(10_000);
        editors.set_vexpand(true);

        let preview = webkit6::WebView::new();
        let settings = webkit6::Settings::new();
        settings.set_enable_javascript(false);
        settings.set_enable_javascript_markup(false);
        settings.set_allow_file_access_from_file_urls(true);
        settings.set_auto_load_images(true);
        settings.set_enable_media(true);
        settings.set_media_playback_requires_user_gesture(true);
        preview.set_settings(&settings);
        preview.set_hexpand(true);
        preview.set_vexpand(true);
        preview.set_width_request(280);

        let outline_list = gtk4::ListBox::new();
        outline_list.add_css_class("navigation-sidebar");
        outline_list.set_activate_on_single_click(true);
        let search_entry = gtk4::SearchEntry::new();
        search_entry.set_placeholder_text(Some("Поиск в запиненном каталоге"));
        let search_list = gtk4::ListBox::new();
        let search_box = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        search_box.add_css_class("side-search");
        search_box.append(&search_entry);
        search_box.append(&scrolled(&search_list));
        let backlink_list = gtk4::ListBox::new();
        backlink_list.set_visible(false);
        let incoming_list = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let outgoing_list = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let incoming_fold = gtk4::Expander::new(Some("Входящие"));
        incoming_fold.set_expanded(true);
        incoming_fold.set_hexpand(true);
        incoming_fold.set_child(Some(&incoming_list));
        let outgoing_fold = gtk4::Expander::new(Some("Исходящие"));
        outgoing_fold.set_expanded(true);
        outgoing_fold.set_hexpand(true);
        outgoing_fold.set_child(Some(&outgoing_list));
        let link_caption = gtk4::Label::new(Some("Тег"));
        link_caption.add_css_class("side-caption");
        link_caption.set_halign(gtk4::Align::Start);
        link_caption.set_hexpand(true);
        let link_clear = gtk4::Button::with_label("К ссылкам");
        link_clear.add_css_class("flat");
        link_clear.set_visible(false);
        let link_head = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        link_head.set_visible(false);
        link_head.append(&link_caption);
        link_head.append(&link_clear);
        let backlink_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        backlink_page.add_css_class("links-page");
        backlink_page.append(&link_head);
        backlink_page.append(&backlink_list);
        backlink_page.append(&incoming_fold);
        backlink_page.append(&outgoing_fold);
        let graph_area = gtk4::DrawingArea::new();
        graph_area.set_hexpand(true);
        graph_area.set_vexpand(true);
        graph_area.set_content_width(280);
        graph_area.set_content_height(360);
        let right = notebook_for(
            seams::PanelSide::Right,
            &[
                ("outline", scrolled(&outline_list).upcast()),
                ("search", search_box.upcast()),
                ("backlinks", scrolled(&backlink_page).upcast()),
                ("graph", graph_area.clone().upcast()),
            ],
        );
        right.set_width_request(220);
        right.set_hexpand(true);
        right.set_vexpand(true);
        right.add_css_class("sidebar-notebook");
        tip_tab(&right, 0, "Заголовки открытого файла");
        tip_tab(&right, 1, "Поиск по запиненному пространству");
        tip_tab(
            &right,
            2,
            "Входящие и исходящие ссылки, файлы с выбранным тегом",
        );
        tip_tab(&right, 3, "Связи открытого файла внутри пространства");
        let doc_kicker = gtk4::Label::new(Some("О файле"));
        doc_kicker.add_css_class("dim-label");
        doc_kicker.set_halign(gtk4::Align::Start);
        doc_kicker.set_hexpand(true);
        doc_kicker.set_margin_start(10);
        let info_pin = dock_pin("Закрепить сведения о файле. Без булавки они схлопываются сами");
        let info_fold = dock_icon("pan-end-symbolic", "Свернуть сведения о файле");
        let doc_head = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        doc_head.set_margin_top(4);
        doc_head.append(&doc_kicker);
        doc_head.append(&info_pin);
        doc_head.append(&info_fold);
        let doc_label = gtk4::Label::new(Some("Файл не открыт"));
        doc_label.add_css_class("heading");
        doc_label.set_halign(gtk4::Align::Start);
        doc_label.set_margin_start(10);
        doc_label.set_margin_bottom(4);
        doc_label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        let tag_box = gtk4::FlowBox::new();
        tag_box.set_selection_mode(gtk4::SelectionMode::None);
        tag_box.set_homogeneous(false);
        tag_box.set_max_children_per_line(8);
        tag_box.set_column_spacing(4);
        tag_box.set_row_spacing(4);
        tag_box.set_margin_start(8);
        tag_box.set_margin_end(8);
        tag_box.set_margin_bottom(4);
        tag_box.set_visible(false);
        let file_facts = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
        file_facts.add_css_class("file-facts");
        file_facts.set_hexpand(true);
        let doc_side = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        doc_side.add_css_class("doc-side");
        doc_side.set_vexpand(true);
        doc_side.set_hexpand(false);
        doc_side.append(&doc_head);
        doc_side.append(&doc_label);
        doc_side.append(&tag_box);
        doc_side.append(&file_facts);
        doc_side.append(&right);
        search_list.add_css_class("navigation-sidebar");
        search_list.add_css_class("file-list");
        backlink_list.add_css_class("navigation-sidebar");
        backlink_list.add_css_class("file-list");
        backlink_list.set_activate_on_single_click(true);

        let find_entry = gtk4::SearchEntry::new();
        find_entry.set_placeholder_text(Some("Найти в файле"));
        find_entry.set_hexpand(true);
        let find_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        find_bar.add_css_class("findbar");
        find_bar.set_visible(false);
        find_bar.append(&find_entry);
        let edit_column = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        edit_column.append(&find_bar);
        edit_column.append(&editors);
        let content_stack = gtk4::Stack::new();
        content_stack.set_hexpand(true);
        content_stack.set_vexpand(true);
        content_stack.add_named(&edit_column, Some("edit"));
        content_stack.add_named(&preview, Some("preview"));
        let picture = gtk4::Picture::new();
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk4::ContentFit::Contain);
        content_stack.add_named(&picture, Some("picture"));
        let folder_dir = gtk4::DirectoryList::new(Some(FOLDER_ATTRS), None::<&gio::File>);
        folder_dir.set_monitored(true);
        let folder_view = folder_columns(&folder_dir);
        let folder_up = gtk4::Button::from_icon_name("go-up-symbolic");
        folder_up.add_css_class("flat");
        folder_up.set_tooltip_text(Some("Вверх"));
        folder_up.set_sensitive(false);
        let folder_crumbs = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        folder_crumbs.set_halign(gtk4::Align::Start);
        let crumbs_scroll = gtk4::ScrolledWindow::new();
        crumbs_scroll.set_child(Some(&folder_crumbs));
        crumbs_scroll.set_hexpand(true);
        crumbs_scroll.set_policy(gtk4::PolicyType::Automatic, gtk4::PolicyType::Never);
        crumbs_scroll.set_propagate_natural_height(true);
        let folder_bar = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        folder_bar.add_css_class("explorer-bar");
        folder_bar.append(&folder_up);
        folder_bar.append(&crumbs_scroll);
        let folder_page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        folder_page.add_css_class("explorer");
        folder_page.append(&folder_bar);
        folder_page.append(&scrolled(&folder_view));
        content_stack.add_named(&folder_page, Some("folder"));
        let mode_button = gtk4::Button::from_icon_name("view-reveal-symbolic");
        mode_button.add_css_class("osd");
        mode_button.add_css_class("circular");
        mode_button.add_css_class("mode-toggle");
        mode_button.set_halign(gtk4::Align::End);
        mode_button.set_valign(gtk4::Align::Start);
        mode_button.set_margin_top(8);
        mode_button.set_margin_end(8);
        mode_button.set_can_focus(false);
        mode_button.set_tooltip_text(Some("Просмотр"));
        if let Some(image) = mode_button
            .child()
            .and_then(|child| child.downcast::<gtk4::Image>().ok())
        {
            image.set_pixel_size(20);
        }
        let (info_rail, info_expand, info_rail_pin) = dock_rail(
            "pan-start-symbolic",
            "Показать сведения о файле",
            "Закрепить сведения о файле. Без булавки они схлопываются сами",
        );
        let info_shell = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        doc_side.set_hexpand(false);
        info_shell.set_hexpand(false);
        info_shell.append(&doc_side);
        info_shell.append(&info_rail);
        let content = gtk4::Overlay::new();
        content.set_child(Some(&content_stack));
        content.add_overlay(&mode_button);
        content.set_hexpand(true);
        content.set_vexpand(true);

        let main = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        let right_paned = gtk4::Paned::new(gtk4::Orientation::Horizontal);
        right_paned.set_start_child(Some(&content));
        right_paned.set_end_child(Some(&info_shell));
        right_paned.set_resize_start_child(true);
        right_paned.set_resize_end_child(false);
        right_paned.set_shrink_start_child(false);
        right_paned.set_shrink_end_child(false);
        right_paned.set_overflow(gtk4::Overflow::Hidden);
        main.set_start_child(Some(&left_shell));
        main.set_end_child(Some(&right_paned));
        main.set_position(280);
        main.set_resize_start_child(true);
        main.set_resize_end_child(true);
        main.set_shrink_start_child(false);
        main.set_shrink_end_child(false);
        main.set_overflow(gtk4::Overflow::Hidden);
        main.add_css_class("workspace-main");

        let pin_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        pin_box.add_css_class("pinbar");
        let pins_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        pins_box.set_hexpand(true);
        let open_mode_dropdown = gtk4::DropDown::from_strings(&["Редактирование", "Просмотр"]);
        if session.open_mode == ViewMode::Preview {
            open_mode_dropdown.set_selected(1);
        }
        open_mode_dropdown.set_tooltip_text(Some(
            "Как открывать следующий файл. Кнопка в углу переключает вид текущего.",
        ));
        let open_mode_label = gtk4::Label::new(Some("Открывать"));
        open_mode_label.add_css_class("dim-label");
        let open_mode_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        open_mode_box.set_halign(gtk4::Align::End);
        open_mode_box.set_valign(gtk4::Align::Center);
        open_mode_box.append(&open_mode_label);
        open_mode_box.append(&open_mode_dropdown);
        pin_box.append(&pins_box);
        pin_box.append(&open_mode_box);
        let path_label = gtk4::Label::new(None);
        path_label.set_halign(gtk4::Align::Start);
        path_label.set_hexpand(true);
        path_label.set_ellipsize(gtk4::pango::EllipsizeMode::Start);
        path_label.set_margin_start(8);
        path_label.add_css_class("path-label");
        let back_button = gtk4::Button::from_icon_name("go-previous-symbolic");
        back_button.add_css_class("flat");
        back_button.set_tooltip_text(Some("Назад: файл, ссылка или каталог"));
        back_button.set_sensitive(false);
        let forward_button = gtk4::Button::from_icon_name("go-next-symbolic");
        forward_button.add_css_class("flat");
        forward_button.set_tooltip_text(Some("Вперёд по тем же переходам"));
        forward_button.set_sensitive(false);
        let pin_file_button = gtk4::Button::from_icon_name("view-pin-symbolic");
        pin_file_button.add_css_class("flat");
        pin_file_button.set_tooltip_text(Some("Закрепить открытый файл"));
        pin_file_button.set_sensitive(false);
        let catalog_button = gtk4::Button::with_label("Каталог");
        catalog_button.add_css_class("flat");
        catalog_button.set_tooltip_text(Some("Каталог открытого файла"));
        catalog_button.set_margin_end(4);
        let path_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        path_row.add_css_class("pathbar");
        path_row.append(&back_button);
        path_row.append(&forward_button);
        path_row.append(&path_label);
        path_row.append(&pin_file_button);
        path_row.append(&catalog_button);
        let status = gtk4::Label::new(Some(""));
        status.set_halign(gtk4::Align::Start);
        status.add_css_class("dim-label");
        status.add_css_class("statusbar");

        let menubar = gtk4::PopoverMenuBar::from_model(None::<&gio::MenuModel>);
        let file_column = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        file_column.set_hexpand(true);
        file_column.set_vexpand(true);
        right_paned.set_start_child(None::<&gtk4::Widget>);
        file_column.append(&path_row);
        file_column.append(&content);
        right_paned.set_start_child(Some(&file_column));
        let root = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        root.append(&pin_box);
        root.append(&main);
        root.append(&status);

        let header = gtk4::HeaderBar::new();
        let (window_width, window_height) = fitted_size(session.window_width, session.window_height);
        let window = gtk4::ApplicationWindow::builder()
            .application(app)
            .title(APP_TITLE)
            .default_width(window_width)
            .default_height(window_height)
            .child(&root)
            .titlebar(&header)
            .show_menubar(true) // единственная строка «Файл / Правка / Вид», в заголовке окна
            .build();
        window.add_css_class("verst");
        window.set_size_request(session::MIN_WINDOW_WIDTH, session::MIN_WINDOW_HEIGHT);
        publish_window_icon(&window);

        let css = gtk4::CssProvider::new();
        gtk4::style_context_add_provider_for_display(
            &gtk4::prelude::WidgetExt::display(&window),
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        let mode_css = gtk4::CssProvider::new();
        mode_css.load_from_string(CHROME_CSS);
        gtk4::style_context_add_provider_for_display(
            &gtk4::prelude::WidgetExt::display(&window),
            &mode_css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let widgets = Widgets {
            window,
            pins_box,
            open_mode_dropdown,
            workspace_slot,
            open_list,
            pinned_list,
            history_list,
            notes_host,
            notes_tags,
            notes_tags_fold,
            notes_tags_scroll,
            notes_tags_all,
            notes_new,
            notes_menu,
            notes_group_flat,
            notes_group_created,
            notes_group_modified,
            notes_group_tags,
            main_paned: main,
            right_paned,
            nav_pin,
            nav_fold,
            activity_bar,
            activity_buttons,
            nav_expand,
            nav_rail,
            nav_rail_pin,
            info_pin,
            info_fold,
            info_expand,
            info_rail,
            info_rail_pin,
            doc_side,
            views,
            second_scroll,
            preview,
            picture,
            outline_list,
            file_facts,
            search_entry,
            search_list,
            backlink_list,
            incoming_list,
            outgoing_list,
            incoming_fold,
            outgoing_fold,
            link_head,
            link_caption,
            link_clear,
            tag_box,
            graph: graph_area,
            left_notebook: left,
            right_notebook: right,
            menubar,
            editor_paned: editors,
            disk_list: Rc::new(RefCell::new(None)),
            workspace_list: Rc::new(RefCell::new(None)),
            path_label,
            nav_path,
            back_button,
            forward_button,
            doc_label,
            catalog_button,
            pin_file_button,
            find_bar,
            find_entry,
            content_stack,
            folder_dir,
            folder_view,
            folder_crumbs,
            folder_up,
            mode_button,
            status,
            css,
            empty_buffer,
            preferences: Rc::new(RefCell::new(None)),
            zoom_spin: Rc::new(RefCell::new(None)),
            open_mode_settings: Rc::new(RefCell::new(None)),
            word_wrap_switch: Rc::new(RefCell::new(None)),
        };
        let ctx = Ctx {
            state: Rc::new(RefCell::new(State {
                view_mode: session.view_mode,
                session,
                buffers: HashMap::new(),
                index: index::LinkIndex::default(),
                index_gen: 0,
                scope_graph: None,
                scope_building: None,
                loading: false,
                dock_adjust: false,
                conflict_open: false,
                autosave: HashMap::new(),
                graph: graph::GraphLayout {
                    nodes: Vec::new(),
                    edges: Vec::new(),
                },
                headings: Vec::new(),
                search_hits: Vec::new(),
                backlinks: Vec::new(),
                tag_query: None,
                nav: places::NavHistory::default(),
                nav_dir: None,
                browsing: false,
                mode_tick: 0,
            })),
            widgets,
        };
        ctx.install_actions(app);
        ctx.bind();
        ctx.mount_disk();
        ctx.rebuild_pins();
        ctx.mount_workspace();
        ctx.apply_scheme(&ctx.widgets.empty_buffer);
        ctx
    }

    fn install_actions(&self, app: &gtk4::Application) {
        let menu = gio::Menu::new();
        let mut sections: Vec<&str> = Vec::new();
        for command in seams::commands() {
            if !sections.contains(&command.section) {
                sections.push(command.section);
            }
        }
        for section in sections {
            let submenu = gio::Menu::new();
            for command in seams::commands()
                .iter()
                .filter(|item| item.section == section)
            {
                submenu.append(Some(command.title), Some(&format!("app.{}", command.id)));
            }
            menu.append_submenu(Some(section), &submenu);
        }
        app.set_menubar(Some(&menu));
        self.widgets.menubar.set_menu_model(Some(&menu));
        for command in seams::commands() {
            app.set_accels_for_action(&format!("app.{}", command.id), command.accels);
        }

        self.action(app, "save", |ctx| ctx.save_focused());
        self.action(app, "quick_note", |ctx| ctx.quick_note());
        self.action(app, "pin", |ctx| ctx.choose_pin());
        self.action(app, "pin_file", |ctx| {
            if let Some(path) = ctx.focused_path() {
                ctx.toggle_pin_file(&path);
            } else {
                ctx.set_status("Нет открытого файла");
            }
        });
        self.action(app, "close_file", |ctx| {
            if let Some(path) = ctx.focused_path() {
                ctx.close_file(&path);
            }
        });
        self.action(app, "preferences", |ctx| ctx.show_preferences());
        self.action(app, "quit", |ctx| ctx.widgets.window.close());
        self.action(app, "undo", |ctx| ctx.edit_undo());
        self.action(app, "redo", |ctx| ctx.edit_redo());
        self.action(app, "cut", |ctx| ctx.focused_view().emit_cut_clipboard());
        self.action(app, "copy", |ctx| {
            if !ctx.copy_focused_tree_path() {
                ctx.focused_view().emit_copy_clipboard();
            }
        });
        self.action(app, "paste", |ctx| {
            ctx.focused_view().emit_paste_clipboard()
        });
        self.action(app, "select_all", |ctx| ctx.edit_select_all());
        self.action(app, "back", |ctx| ctx.shift_history(true));
        self.action(app, "forward", |ctx| ctx.shift_history(false));
        self.action(app, "show_history", |ctx| ctx.show_panel("history"));
        self.action(app, "split", |ctx| ctx.toggle_split());
        let ctx = self.clone();
        let word_wrap = gio::SimpleAction::new_stateful(
            "word_wrap",
            None,
            &self.state.borrow().session.word_wrap.to_variant(),
        );
        word_wrap.connect_activate(move |action, _| {
            let current = action
                .state()
                .and_then(|state| state.get::<bool>())
                .unwrap_or(false);
            ctx.set_word_wrap(!current);
        });
        app.add_action(&word_wrap);
        self.action(app, "zoom_in", |ctx| ctx.bump_zoom(1));
        self.action(app, "zoom_out", |ctx| ctx.bump_zoom(-1));
        self.action(app, "search", |ctx| ctx.show_panel("search"));
        self.action(app, "find", |ctx| ctx.show_find());
        self.action(app, "quick_open", |ctx| ctx.quick_open());
        self.action(app, "reveal_disk", |ctx| {
            ctx.reveal_focused(TreePlace::Disk)
        });
        self.action(app, "reveal_workspace", |ctx| {
            ctx.reveal_focused(TreePlace::Workspace)
        });
        self.action(app, "browse_folder", |ctx| ctx.browse_focused());
        self.action(app, "browse_up", |ctx| {
            if ctx.state.borrow().browsing {
                ctx.folder_up();
            } else {
                ctx.browse_focused();
            }
        });
        app.set_accels_for_action("app.browse_up", &["<Alt>Up"]);
        self.action(app, "next_space", |ctx| ctx.cycle_space(true));
        self.action(app, "previous_space", |ctx| ctx.cycle_space(false));
        let ctx = self.clone();
        let space_index = gio::SimpleAction::new("space_index", Some(glib::VariantTy::INT32));
        space_index.connect_activate(move |_, param| {
            let Some(index) = param.and_then(|value| value.get::<i32>()) else {
                return;
            };
            if index < 0 {
                return;
            }
            ctx.activate_space_at(index as usize);
        });
        app.add_action(&space_index);
        for number in 1..=9 {
            app.set_accels_for_action(
                &format!("app.space_index({})", number - 1),
                &[
                    &format!("<Control>{number}"),
                    &format!("<Control>KP_{number}"),
                ],
            );
        }
    }

    fn action(&self, app: &gtk4::Application, name: &str, run: impl Fn(Ctx) + 'static) {
        let ctx = self.clone();
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| run(ctx.clone()));
        app.add_action(&action);
    }

    fn bind(&self) {
        let ctx = self.clone();
        self.widgets.window.connect_close_request(move |_| {
            ctx.save_session();
            glib::Propagation::Proceed
        });
        let ctx = self.clone();
        self.widgets
            .left_notebook
            .connect_switch_page(move |_, page, page_index| {
                let mut state = ctx.state.borrow_mut();
                let id = page.widget_name();
                if !id.is_empty() {
                    state.session.left_panel = id.to_string();
                }
                state.session.left_page = page_index;
                let notes = id == "notes";
                drop(state);
                ctx.state.borrow_mut().nav_dir = None;
                ctx.save_session();
                ctx.sync_activity();
                ctx.refresh_nav_path();
                if notes {
                    ctx.refresh_notes();
                }
            });
        let ctx = self.clone();
        self.widgets
            .right_notebook
            .connect_switch_page(move |_, page, page_index| {
                let mut state = ctx.state.borrow_mut();
                let id = page.widget_name();
                if !id.is_empty() {
                    state.session.right_panel = id.to_string();
                }
                state.session.right_page = page_index;
                drop(state);
                ctx.save_session();
            });
        let ctx = self.clone();
        self.widgets
            .left_notebook
            .connect_page_reordered(move |notebook, _, _| {
                if ctx.state.borrow().loading {
                    return;
                }
                let order = notebook_order(notebook);
                ctx.state.borrow_mut().session.left_order = order;
                ctx.save_session();
            });
        let ctx = self.clone();
        self.widgets
            .right_notebook
            .connect_page_reordered(move |notebook, _, _| {
                if ctx.state.borrow().loading {
                    return;
                }
                let order = notebook_order(notebook);
                ctx.state.borrow_mut().session.right_order = order;
                ctx.save_session();
            });

        let ctx = self.clone();
        self.widgets.notes_new.connect_clicked(move |_| ctx.quick_note());
        let ctx = self.clone();
        self.widgets
            .notes_group_flat
            .connect_clicked(move |_| ctx.set_note_group("flat"));
        let ctx = self.clone();
        self.widgets
            .notes_group_created
            .connect_clicked(move |_| ctx.set_note_group("created"));
        let ctx = self.clone();
        self.widgets
            .notes_group_modified
            .connect_clicked(move |_| ctx.set_note_group("modified"));
        let ctx = self.clone();
        self.widgets
            .notes_group_tags
            .connect_clicked(move |_| ctx.set_note_group("tags"));
        let ctx = self.clone();
        self.widgets
            .notes_tags_fold
            .connect_expanded_notify(move |fold| {
                if ctx.state.borrow().loading {
                    return;
                }
                ctx.state.borrow_mut().session.note_tags_open = fold.is_expanded();
                ctx.save_session();
            });
        let ctx = self.clone();
        self.widgets.notes_tags_all.connect_clicked(move |button| {
            let show_all = button.label().as_deref() != Some("Три строки");
            let rows = if show_all { 5 } else { 3 };
            ctx.widgets
                .notes_tags_scroll
                .set_max_content_height(rows * 36);
            button.set_label(if show_all {
                "Три строки"
            } else {
                "Все теги"
            });
        });
        for button in self.widgets.activity_buttons.clone() {
            let ctx = self.clone();
            let id = button.widget_name().to_string();
            button.connect_clicked(move |button| {
                for other in &ctx.widgets.activity_buttons {
                    other.remove_css_class("activity-on");
                }
                button.add_css_class("activity-on");
                ctx.state.borrow_mut().session.left_panel = id.clone();
                ctx.show_panel(&id);
                if !ctx.state.borrow().session.nav_open {
                    ctx.expand_nav();
                }
                ctx.sync_activity();
            });
        }
        let ctx = self.clone();
        self.widgets.nav_pin.connect_toggled(move |button| {
            ctx.set_nav_pinned(button.is_active());
        });
        let ctx = self.clone();
        self.widgets.nav_rail_pin.connect_toggled(move |button| {
            ctx.set_nav_pinned(button.is_active());
        });
        let ctx = self.clone();
        self.widgets.nav_fold.connect_clicked(move |_| ctx.fold_nav());
        let ctx = self.clone();
        self.widgets.nav_expand.connect_clicked(move |_| ctx.expand_nav());
        let ctx = self.clone();
        self.widgets.info_pin.connect_toggled(move |button| {
            ctx.set_info_pinned(button.is_active());
        });
        let ctx = self.clone();
        self.widgets
            .info_rail_pin
            .connect_toggled(move |button| ctx.set_info_pinned(button.is_active()));
        let ctx = self.clone();
        self.widgets.info_fold.connect_clicked(move |_| ctx.fold_info());
        let ctx = self.clone();
        self.widgets
            .info_expand
            .connect_clicked(move |_| ctx.expand_info());
        let ctx = self.clone();
        self.widgets
            .main_paned
            .connect_position_notify(move |paned| {
                if ctx.state.borrow().loading || ctx.state.borrow().dock_adjust {
                    return;
                }
                if !ctx.state.borrow().session.nav_open {
                    if paned.position() != session::ACTIVITY_WIDTH {
                        ctx.place_paned(paned, session::ACTIVITY_WIDTH);
                    }
                    return;
                }
                let position = paned.position();
                let floor = ctx.nav_floor(paned);
                if position < floor {
                    ctx.place_paned(paned, floor);
                    return;
                }
                if !(session::NAV_MIN_WIDTH..=session::NAV_MAX_WIDTH).contains(&position) {
                    return;
                }
                ctx.state.borrow_mut().session.nav_width = position;
                ctx.save_session();
            });
        let ctx = self.clone();
        self.widgets
            .right_paned
            .connect_position_notify(move |paned| {
                if ctx.state.borrow().loading || ctx.state.borrow().dock_adjust {
                    return;
                }
                if !ctx.state.borrow().session.info_open {
                    let total = paned.width();
                    if total > 80 {
                        let target = total - session::DOCK_RAIL_WIDTH;
                        if (paned.position() - target).abs() > 6 {
                            ctx.place_paned(paned, target);
                        }
                    }
                    return;
                }
                let total = paned.width();
                if total < 200 {
                    return;
                }
                let width = total - paned.position();
                let floor = ctx.info_floor(paned);
                if width < floor {
                    ctx.place_paned(paned, total - floor);
                    return;
                }
                if !(session::INFO_MIN_WIDTH..=session::INFO_MAX_WIDTH).contains(&width) {
                    return;
                }
                ctx.state.borrow_mut().session.info_width = width;
                ctx.save_session();
            });
        let ctx = self.clone();
        self.widgets.right_paned.connect_map(move |_| {
            let ctx = ctx.clone();
            glib::idle_add_local_once(move || ctx.apply_info_dock());
        });

        for pane in 0..2 {
            let ctx = self.clone();
            let focus = gtk4::EventControllerFocus::new();
            focus.connect_enter(move |_| {
                ctx.state.borrow_mut().session.focused_pane = pane;
                ctx.follow_focus();
                ctx.autohide_docks();
            });
            self.widgets.views[pane].add_controller(focus);
        }
        let ctx = self.clone();
        let click = gtk4::EventControllerLegacy::new();
        click.set_propagation_phase(gtk4::PropagationPhase::Capture);
        click.connect_event(move |_, event| {
            if event.downcast_ref::<gtk4::gdk::ButtonEvent>().is_some() {
                ctx.autohide_docks();
            }
            glib::Propagation::Proceed
        });
        self.widgets.content_stack.add_controller(click);

        let ctx = self.clone();
        self.widgets.open_list.connect_row_activated(move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            let path = ctx
                .state
                .borrow()
                .session
                .open_files
                .get(index as usize)
                .cloned();
            if let Some(path) = path {
                ctx.go(path, None);
            }
        });

        let ctx = self.clone();
        self.widgets
            .pinned_list
            .connect_row_activated(move |_, row| {
                let index = row.index();
                if index < 0 {
                    return;
                }
                let path = ctx
                    .state
                    .borrow()
                    .session
                    .pinned_files
                    .get(index as usize)
                    .cloned();
                if let Some(path) = path {
                    ctx.go(path, None);
                }
            });

        let ctx = self.clone();
        self.widgets
            .history_list
            .connect_row_activated(move |_, row| {
                let index = row.index();
                if index < 0 {
                    return;
                }
                let path = ctx
                    .state
                    .borrow()
                    .session
                    .recent_files
                    .get(index as usize)
                    .cloned();
                if let Some(path) = path {
                    ctx.go(path, None);
                }
            });

        let ctx = self.clone();
        self.widgets
            .outline_list
            .connect_row_activated(move |_, row| ctx.jump_heading(row.index()));

        let ctx = self.clone();
        self.widgets
            .search_list
            .connect_row_activated(move |_, row| {
                let index = row.index();
                if index < 0 {
                    return;
                }
                let hit = ctx.state.borrow().search_hits.get(index as usize).cloned();
                if let Some(hit) = hit {
                    ctx.go(hit.path, Some(hit.line as u32));
                }
            });

        let ctx = self.clone();
        self.widgets
            .backlink_list
            .connect_row_activated(move |_, row| {
                let index = row.index();
                if index < 0 {
                    return;
                }
                let path = ctx.state.borrow().backlinks.get(index as usize).cloned();
                if let Some(path) = path {
                    ctx.go(path, None);
                }
            });

        let ctx = self.clone();
        self.widgets.link_clear.connect_clicked(move |_| {
            ctx.state.borrow_mut().tag_query = None;
            ctx.refresh_backlinks();
            ctx.refresh_tag_chips();
        });

        let ctx = self.clone();
        self.widgets
            .search_entry
            .connect_search_changed(move |_| ctx.schedule_search());
        let ctx = self.clone();
        self.widgets
            .search_entry
            .connect_activate(move |_| ctx.run_search());

        let ctx = self.clone();
        self.widgets
            .open_mode_dropdown
            .connect_selected_notify(move |dropdown| {
                let mode = if dropdown.selected() == 1 {
                    ViewMode::Preview
                } else {
                    ViewMode::Edit
                };
                ctx.set_open_mode(mode);
            });
        let ctx = self.clone();
        self.widgets
            .mode_button
            .connect_clicked(move |_| ctx.toggle_view_mode());
        let ctx = self.clone();
        self.widgets.menubar.set_visible(false);
        self.widgets.catalog_button.connect_clicked(move |button| {
            ctx.show_catalog_menu(button);
        });
        let ctx = self.clone();
        self.widgets
            .folder_up
            .connect_clicked(move |_| ctx.folder_up());
        let ctx = self.clone();
        self.widgets
            .folder_view
            .connect_activate(move |_, position| ctx.open_folder_item(position));
        let ctx = self.clone();
        let folder_keys = gtk4::EventControllerKey::new();
        folder_keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk4::gdk::Key::BackSpace && ctx.state.borrow().browsing {
                ctx.folder_up();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.widgets.folder_view.add_controller(folder_keys);
        let ctx = self.clone();
        self.widgets
            .back_button
            .connect_clicked(move |_| ctx.shift_history(true));
        let ctx = self.clone();
        self.widgets
            .forward_button
            .connect_clicked(move |_| ctx.shift_history(false));
        let ctx = self.clone();
        let nav_keys = gtk4::EventControllerKey::new();
        nav_keys.set_propagation_phase(gtk4::PropagationPhase::Capture);
        nav_keys.connect_key_pressed(move |_, key, _, modifier| {
            let alt = modifier.contains(gtk4::gdk::ModifierType::ALT_MASK);
            if !alt || !ctx.preview_focused() {
                return glib::Propagation::Proceed;
            }
            if key == gtk4::gdk::Key::Left {
                ctx.shift_history(true);
                return glib::Propagation::Stop;
            }
            if key == gtk4::gdk::Key::Right {
                ctx.shift_history(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.widgets.window.add_controller(nav_keys);
        let ctx = self.clone();
        let nav_click = gtk4::GestureClick::new();
        nav_click.set_button(0);
        nav_click.set_propagation_phase(gtk4::PropagationPhase::Capture);
        nav_click.connect_pressed(move |gesture, _, _, _| match gesture.current_button() {
            8 => {
                ctx.shift_history(true);
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            }
            9 => {
                ctx.shift_history(false);
                gesture.set_state(gtk4::EventSequenceState::Claimed);
            }
            _ => {}
        });
        self.widgets.window.add_controller(nav_click);
        let ctx = self.clone();
        self.widgets.pin_file_button.connect_clicked(move |_| {
            if let Some(path) = ctx.focused_path() {
                ctx.toggle_pin_file(&path);
            }
        });
        let ctx = self.clone();
        self.widgets
            .find_entry
            .connect_activate(move |_| ctx.find_move(true));
        let ctx = self.clone();
        let find_keys = gtk4::EventControllerKey::new();
        find_keys.connect_key_pressed(move |_, key, _, modifier| {
            if key == gtk4::gdk::Key::Escape {
                ctx.hide_find();
                return glib::Propagation::Stop;
            }
            if key == gtk4::gdk::Key::Return
                && modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK)
            {
                ctx.find_move(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.widgets.find_entry.add_controller(find_keys);

        let ctx = self.clone();
        self.widgets
            .preview
            .connect_decide_policy(move |_, decision, kind| {
                if kind != webkit6::PolicyDecisionType::NavigationAction {
                    return false;
                }
                let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
                    return false;
                };
                let Some(mut action) = nav.navigation_action() else {
                    return false;
                };
                let Some(request) = action.request() else {
                    return false;
                };
                let Some(uri) = request.uri() else {
                    return false;
                };
                // load_uri сам открывает pdf и картинку. Клик по file: из страницы по-прежнему глушим.
                let navigation = action.navigation_type();
                if uri.starts_with("file:")
                    && navigation != webkit6::NavigationType::LinkClicked
                    && navigation != webkit6::NavigationType::FormSubmitted
                {
                    return false;
                }
                ctx.handle_uri(&uri, decision)
            });

        let state = self.state.clone();
        self.widgets.graph.set_draw_func(move |area, cr, _, _| {
            let layout = state.borrow().graph.clone();
            paint_graph(area, cr, &layout);
        });
        let ctx = self.clone();
        self.widgets
            .graph
            .connect_resize(move |_, _, _| ctx.relayout_graph());
        let ctx = self.clone();
        let click = gtk4::GestureClick::new();
        click.connect_released(move |_, _, x, y| {
            let path = ctx
                .state
                .borrow()
                .graph
                .hit(x, y)
                .map(|node| node.path.clone());
            if let Some(path) = path {
                ctx.go(path, None);
            }
        });
        self.widgets.graph.add_controller(click);
    }

    fn mount_disk(&self) {
        let (scroll, list) = file_tree(Path::new("/"), self.tree_actions());
        self.watch_tree_directory(&list);
        *self.widgets.disk_list.borrow_mut() = Some(list);
        let index = notebook_index_of(&self.widgets.left_notebook, "disk").unwrap_or(0);
        replace_notebook_page(&self.widgets.left_notebook, index, &scroll);
    }

    fn mount_workspace(&self) {
        clear_box(&self.widgets.workspace_slot);
        let Some(pin) = self.pin_path() else {
            *self.widgets.workspace_list.borrow_mut() = None;
            self.refresh_nav_path();
            let label = gtk4::Label::new(Some(
                "Запините каталог из меню файла или правой кнопкой в дереве",
            ));
            label.set_margin_top(12);
            label.set_wrap(true);
            label.set_margin_start(8);
            label.set_margin_end(8);
            self.widgets.workspace_slot.append(&label);
            return;
        };
        let (scroll, list) = file_tree(&pin, self.tree_actions());
        self.watch_tree_directory(&list);
        *self.widgets.workspace_list.borrow_mut() = Some(list);
        scroll.set_vexpand(true);
        self.widgets.workspace_slot.append(&scroll);
        self.refresh_nav_path();
    }

    fn reveal_focused(&self, place: TreePlace) {
        let Some(path) = self.focused_path() else {
            self.set_status("Нет открытого файла");
            return;
        };
        self.reveal_path(path, place);
    }

    fn reveal_path(&self, path: PathBuf, place: TreePlace) {
        match place {
            TreePlace::Disk => {
                self.show_panel("disk");
                let Some(list) = self.widgets.disk_list.borrow().clone() else {
                    return;
                };
                reveal_in_tree(list, PathBuf::from("/"), path, self.tree_notice());
            }
            TreePlace::Workspace => {
                let Some(pin) = self.pin_path() else {
                    self.set_status("Сначала запините пространство");
                    return;
                };
                if !path.starts_with(&pin) {
                    self.set_status("Файл лежит вне запиненного пространства");
                    return;
                }
                self.show_panel("workspace");
                let Some(list) = self.widgets.workspace_list.borrow().clone() else {
                    self.set_status("Дерево пространства ещё не построено");
                    return;
                };
                reveal_in_tree(list, pin, path, self.tree_notice());
            }
        }
    }

    fn tree_actions(&self) -> TreeActions {
        let ctx = self.clone();
        let open = Rc::new(move |path: PathBuf| ctx.go(path, None));
        let ctx = self.clone();
        let pin = Rc::new(move |path: PathBuf| ctx.add_pin(path));
        let ctx = self.clone();
        let notice = Rc::new(move |text: String| ctx.set_status(&text));
        let ctx = self.clone();
        let reveal_disk = Rc::new(move |path: PathBuf| ctx.reveal_path(path, TreePlace::Disk));
        let ctx = self.clone();
        let reveal_workspace =
            Rc::new(move |path: PathBuf| ctx.reveal_path(path, TreePlace::Workspace));
        let ctx = self.clone();
        let pin_file = Rc::new(move |path: PathBuf| ctx.toggle_pin_file(&path));
        let ctx = self.clone();
        let browse = Rc::new(move |path: PathBuf| ctx.show_folder(path));
        let ctx = self.clone();
        let mkdir = Rc::new(move |path: PathBuf| ctx.prompt_directory(path));
        let ctx = self.clone();
        let mkfile = Rc::new(move |path: PathBuf| ctx.prompt_file(path));
        let ctx = self.clone();
        let is_pinned = Rc::new(move |path: &Path| {
            ctx.state
                .borrow()
                .session
                .pinned_files
                .iter()
                .any(|item| item == path)
        });
        TreeActions {
            open,
            pin,
            pin_file,
            is_pinned,
            notice,
            reveal_disk,
            reveal_workspace,
            browse,
            mkdir,
            mkfile,
        }
    }

    fn prompt_directory(&self, parent: PathBuf) {
        let ctx = self.clone();
        self.prompt_entry("Новый каталог", "Имя каталога", parent, move |folder, raw| {
            ctx.make_directory(folder, raw)
        });
    }

    fn prompt_file(&self, parent: PathBuf) {
        let ctx = self.clone();
        self.prompt_entry("Новый файл", "Имя файла", parent, move |folder, raw| {
            ctx.make_file(folder, raw)
        });
    }

    fn prompt_entry(
        &self,
        title: &str,
        placeholder: &str,
        parent: PathBuf,
        on_submit: impl Fn(&Path, &str) -> bool + 'static,
    ) {
        let window = gtk4::Window::builder()
            .transient_for(&self.widgets.window)
            .modal(true)
            .title(title)
            .default_width(420)
            .build();
        window.add_css_class("prompt-dialog");
        let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
        box_.set_margin_top(12);
        box_.set_margin_bottom(12);
        box_.set_margin_start(12);
        box_.set_margin_end(12);
        let hint = gtk4::Label::new(Some(&parent.display().to_string()));
        hint.add_css_class("dim-label");
        hint.set_halign(gtk4::Align::Start);
        hint.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        hint.set_tooltip_text(Some(&parent.display().to_string()));
        let entry = gtk4::Entry::new();
        entry.set_placeholder_text(Some(placeholder));
        entry.set_activates_default(true);
        let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        buttons.set_halign(gtk4::Align::End);
        let cancel = gtk4::Button::with_label("Отмена");
        let create = gtk4::Button::with_label("Создать");
        create.add_css_class("suggested-action");
        buttons.append(&cancel);
        buttons.append(&create);
        box_.append(&hint);
        box_.append(&entry);
        box_.append(&buttons);
        window.set_child(Some(&box_));
        window.set_default_widget(Some(&create));
        let close = window.clone();
        cancel.connect_clicked(move |_| close.close());
        let submit = window.clone();
        let folder = parent.clone();
        let focused = entry.clone();
        create.connect_clicked(move |_| {
            if on_submit(&folder, &entry.text()) {
                submit.close();
            }
        });
        window.present();
        focused.grab_focus();
    }

    fn make_directory(&self, parent: &Path, raw: &str) -> bool {
        let name = match directory_name(raw) {
            Ok(name) => name,
            Err(message) => {
                self.set_status(message);
                return false;
            }
        };
        let target = parent.join(name);
        if target.exists() {
            self.set_status("Такое имя уже есть");
            return false;
        }
        match std::fs::create_dir(&target) {
            Ok(()) => {
                self.set_status("Каталог создан");
                true
            }
            Err(error) => {
                self.set_status(&format!("Не удалось создать каталог: {error}"));
                false
            }
        }
    }

    fn make_file(&self, parent: &Path, raw: &str) -> bool {
        let name = match file_name(raw) {
            Ok(name) => name,
            Err(message) => {
                self.set_status(message);
                return false;
            }
        };
        let target = parent.join(name);
        if target.exists() {
            self.set_status("Такое имя уже есть");
            return false;
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
        {
            Ok(_) => {
                self.set_status("Файл создан");
                self.go(target, None);
                true
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.set_status("Такое имя уже есть");
                false
            }
            Err(error) => {
                self.set_status(&format!("Не удалось создать файл: {error}"));
                false
            }
        }
    }

    fn tree_notice(&self) -> Rc<dyn Fn(String)> {
        let ctx = self.clone();
        Rc::new(move |text| ctx.set_status(&text))
    }

    fn restore(&self) {
        self.state.borrow_mut().loading = true;
        let session = self.state.borrow().session.clone();
        self.apply_zoom(session.zoom_pt);
        self.widgets.second_scroll.set_visible(session.split);
        if let Some(parent) = self.widgets.second_scroll.parent() {
            parent.set_visible(session.split);
        }
        if let Some(path) = session.pane_paths.first().cloned().flatten() {
            self.state.borrow_mut().session.focused_pane = 0;
            self.open_path(path);
        }
        if session.split {
            if let Some(path) = session.pane_paths.get(1).cloned().flatten() {
                self.state.borrow_mut().session.focused_pane = 1;
                self.open_path(path);
            }
        }
        self.state.borrow_mut().session.focused_pane = session.focused_pane.min(1);
        self.rebuild_pins();
        self.mount_workspace();
        apply_notebook_order(&self.widgets.left_notebook, &session.left_order);
        apply_notebook_order(&self.widgets.right_notebook, &session.right_order);
        self.widgets.left_notebook.set_current_page(Some(
            notebook_index_of(&self.widgets.left_notebook, &session.left_panel).unwrap_or(0),
        ));
        self.widgets.right_notebook.set_current_page(Some(
            notebook_index_of(&self.widgets.right_notebook, &session.right_panel).unwrap_or(0),
        ));
        self.apply_nav_dock();
        self.apply_info_dock();
        let focused = self.focused_path();
        let mode = if focused.as_deref().is_some_and(text_workspace::notes::is_inbox) {
            ViewMode::Edit
        } else {
            focused
                .as_deref()
                .and_then(|path| session.mode_of(path))
                .unwrap_or(session.view_mode)
        };
        self.state.borrow_mut().view_mode = mode;
        self.state.borrow_mut().session.view_mode = mode;
        if let Some(path) = focused {
            if text_workspace::notes::is_inbox(&path) {
                self.state
                    .borrow_mut()
                    .session
                    .remember_mode(path, ViewMode::Edit);
            }
        }
        self.reindex();
        self.follow_focus();
        self.refresh_open_list();
        self.refresh_notes();
        self.apply_view_mode();
        self.apply_wrap();
        self.restore_carets(&session);
        self.state.borrow_mut().loading = false;
        self.save_session();
    }

    fn choose_pin(&self) {
        let dialog = gtk4::FileDialog::builder()
            .title("Запинить каталог")
            .modal(true)
            .build();
        let ctx = self.clone();
        let window = self.widgets.window.clone();
        dialog.select_folder(Some(&window), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result {
                if let Some(path) = file.path() {
                    ctx.add_pin(path);
                }
            }
        });
    }

    fn add_pin(&self, path: PathBuf) {
        if !path.is_dir() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            if let Some(index) = state.session.pins.iter().position(|pin| pin == &path) {
                state.session.active_pin = Some(index);
            } else {
                state.session.pins.push(path);
                state.session.active_pin = Some(state.session.pins.len() - 1);
            }
        }
        self.rebuild_pins();
        self.mount_workspace();
        self.reindex();
        self.show_panel("workspace");
        self.save_session();
    }

    fn activate_pin(&self, index: usize) {
        if self.state.borrow().session.pins.get(index).is_none() {
            return;
        }
        self.state.borrow_mut().session.active_pin = Some(index);
        let name = self
            .state
            .borrow()
            .session
            .pins
            .get(index)
            .and_then(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "каталог".to_string());
        self.rebuild_pins();
        self.mount_workspace();
        self.reindex();
        self.show_panel("workspace");
        self.save_session();
        self.set_status(&format!(
            "Пространство «{name}»: дерево, поиск, входящие и граф"
        ));
    }

    fn activate_space_at(&self, index: usize) {
        let len = self.state.borrow().session.pins.len();
        if len == 0 {
            self.set_status("Нет запиненного пространства");
            return;
        }
        if index >= len {
            self.set_status("Нет пространства с таким номером");
            return;
        }
        self.activate_pin(index);
    }

    fn cycle_space(&self, forward: bool) {
        let (len, active) = {
            let session = &self.state.borrow().session;
            (session.pins.len(), session.active_pin)
        };
        let Some(index) = cycle_index(len, active, forward) else {
            self.set_status("Нет запиненного пространства");
            return;
        };
        self.activate_pin(index);
    }

    fn focused_tree(&self) -> Option<gtk4::ListView> {
        let disk = self.widgets.disk_list.borrow().clone();
        let workspace = self.widgets.workspace_list.borrow().clone();
        let mut widget = gtk4::prelude::GtkWindowExt::focus(&self.widgets.window)?;
        loop {
            if let Ok(list) = widget.clone().downcast::<gtk4::ListView>() {
                if disk.as_ref().is_some_and(|item| item == &list)
                    || workspace.as_ref().is_some_and(|item| item == &list)
                {
                    return Some(list);
                }
            }
            widget = widget.parent()?;
        }
    }

    fn copy_focused_tree_path(&self) -> bool {
        let Some(list) = self.focused_tree() else {
            return false;
        };
        let Some(path) = selected_tree_row(&list).as_ref().and_then(row_path) else {
            return false;
        };
        let Some(display) = gtk4::gdk::Display::default() else {
            return false;
        };
        display.clipboard().set_text(&path.display().to_string());
        self.set_status("Путь скопирован");
        true
    }

    fn remove_pin(&self, index: usize) {
        {
            let mut state = self.state.borrow_mut();
            if index >= state.session.pins.len() {
                return;
            }
            state.session.pins.remove(index);
            state.session.active_pin = match state.session.active_pin {
                Some(active) if active == index => {
                    if state.session.pins.is_empty() {
                        None
                    } else {
                        Some(0)
                    }
                }
                Some(active) if active > index => Some(active - 1),
                other => other,
            };
        }
        self.rebuild_pins();
        self.mount_workspace();
        self.reindex();
        self.save_session();
    }

    fn rebuild_pins(&self) {
        clear_box(&self.widgets.pins_box);
        let pins = self.state.borrow().session.pins.clone();
        let active = self.state.borrow().session.active_pin;
        let caption = gtk4::Label::new(Some("Пространства"));
        caption.add_css_class("dim-label");
        caption.set_margin_end(4);
        self.widgets.pins_box.append(&caption);
        if pins.is_empty() {
            let hint = gtk4::Label::new(Some("Запините каталог, чтобы открыть пространство"));
            hint.add_css_class("dim-label");
            hint.set_margin_start(4);
            self.widgets.pins_box.append(&hint);
        }
        for (index, path) in pins.iter().enumerate() {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("каталог");
            let button = gtk4::Button::with_label(name);
            button.add_css_class("pin-name");
            let number = if index < 9 {
                format!("\nCtrl+{}", index + 1)
            } else {
                String::new()
            };
            button.set_tooltip_text(Some(&format!(
                "Открыть пространство «{name}»{number}\n{}",
                path.display()
            )));
            if Some(index) == active {
                button.add_css_class("suggested-action");
            }
            let ctx = self.clone();
            button.connect_clicked(move |_| ctx.activate_pin(index));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.add_css_class("flat");
            close.add_css_class("pin-close");
            let ctx = self.clone();
            close.connect_clicked(move |_| ctx.remove_pin(index));
            let group = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            group.add_css_class("linked");
            group.append(&button);
            group.append(&close);
            self.widgets.pins_box.append(&group);
        }
    }

    fn toggle_split(&self) {
        let split = !self.state.borrow().session.split;
        self.state.borrow_mut().session.split = split;
        self.widgets.second_scroll.set_visible(split);
        if let Some(parent) = self.widgets.second_scroll.parent() {
            parent.set_visible(split);
        }
        if split {
            let height = self.widgets.editor_paned.height();
            if height > 80 {
                self.widgets.editor_paned.set_position(height / 2);
            }
        } else {
            self.widgets.editor_paned.set_position(10_000);
            self.state.borrow_mut().session.focused_pane = 0;
        }
        self.save_session();
        self.set_status(if split {
            "Вторая панель открыта"
        } else {
            "Вторая панель скрыта"
        });
    }

    fn bump_zoom(&self, delta: i32) {
        let next = zoom::step(self.state.borrow().session.zoom_pt, delta);
        self.state.borrow_mut().session.zoom_pt = next;
        self.apply_zoom(next);
        self.save_session();
    }

    fn apply_zoom(&self, pt: i32) {
        self.widgets.css.load_from_string(&zoom::css(pt));
        self.widgets.preview.set_zoom_level(zoom::webkit_factor(pt));
        if let Some(spin) = self.widgets.zoom_spin.borrow().clone() {
            if spin.value() as i32 != pt {
                spin.set_value(f64::from(pt));
            }
        }
        self.set_status("Масштаб окна");
    }

    fn follow_focus(&self) {
        self.state.borrow_mut().nav_dir = None;
        self.apply_saved_mode();
        let path = self.focused_path();
        match &path {
            Some(path) => {
                let shown = path.display().to_string();
                self.widgets.path_label.set_text(&shown);
                self.widgets.path_label.set_tooltip_text(Some(&shown));
                self.widgets.window.set_title(Some(&format!(
                    "{} — {APP_TITLE}",
                    file_label(path)
                )));
                self.widgets.doc_label.set_text(&file_label(path));
                self.widgets
                    .doc_label
                    .set_tooltip_text(Some(&path.display().to_string()));
            }
            None => {
                self.widgets.path_label.set_text("");
                self.widgets.path_label.set_tooltip_text(None);
                self.widgets.window.set_title(Some(APP_TITLE));
                self.widgets.doc_label.set_text("Файл не открыт");
                self.widgets.doc_label.set_tooltip_text(None);
            }
        }
        self.refresh_outline();
        self.refresh_file_facts();
        self.refresh_preview_now();
        self.refresh_backlinks();
        self.relayout_graph();
        self.refresh_open_list();
        self.apply_view_mode();
        self.update_pin_button();
    }

    fn toggle_view_mode(&self) {
        if self.state.borrow().browsing {
            self.state.borrow_mut().browsing = false;
            self.apply_view_mode();
            return;
        }
        if self
            .focused_path()
            .is_some_and(|path| kind::is_view_only(&path))
        {
            self.set_status("Этот файл только для просмотра");
            return;
        }
        let next = match self.state.borrow().view_mode {
            ViewMode::Edit => ViewMode::Preview,
            ViewMode::Preview => ViewMode::Edit,
        };
        self.change_view_mode(next);
    }

    fn change_view_mode(&self, next: ViewMode) {
        if self.state.borrow().view_mode == next {
            return;
        }
        let tick = {
            let mut state = self.state.borrow_mut();
            state.mode_tick += 1;
            state.mode_tick
        };
        let from_preview = self.state.borrow().view_mode == ViewMode::Preview
            && self.focused_path().is_some_and(|path| is_markdown(&path));
        if from_preview {
            let ctx = self.clone();
            self.read_preview_anchor(move |line| {
                if ctx.state.borrow().mode_tick != tick {
                    return;
                }
                ctx.finish_view_mode(next, line);
            });
            return;
        }
        let line = if self.focused_path().is_some_and(|path| is_markdown(&path)) {
            self.editor_anchor_line()
        } else {
            None
        };
        self.finish_view_mode(next, line);
    }

    fn finish_view_mode(&self, next: ViewMode, anchor: Option<u32>) {
        self.state.borrow_mut().view_mode = next;
        self.remember_focused_mode();
        self.apply_view_mode();
        if let Some(line) = anchor {
            self.place_anchor(line, next == ViewMode::Preview);
        }
        self.save_session();
    }

    fn remember_focused_mode(&self) {
        let Some(path) = self.focused_path() else {
            return;
        };
        let mode = self.state.borrow().view_mode;
        self.state.borrow_mut().session.remember_mode(path, mode);
    }

    fn apply_saved_mode(&self) {
        if self.state.borrow().loading || self.state.borrow().browsing {
            return;
        }
        let Some(path) = self.focused_path() else {
            return;
        };
        let Some(mode) = self.state.borrow().session.mode_of(&path) else {
            return;
        };
        if self.state.borrow().view_mode != mode {
            self.state.borrow_mut().view_mode = mode;
        }
    }

    fn place_anchor(&self, line: u32, preview: bool) {
        if preview {
            self.scroll_preview_to_line(line, 0);
            return;
        }
        let ctx = self.clone();
        let mut attempts = 0;
        glib::idle_add_local(move || {
            attempts += 1;
            if ctx.scroll_editor_to_line(line) || attempts > 12 {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }

    fn read_preview_anchor(&self, done: impl FnOnce(Option<u32>) + 'static) {
        let script = r#"(function(){
  var nodes = document.querySelectorAll('[id^="twl"]');
  if (!nodes.length || window.innerHeight < 1) return -1;
  var mark = window.innerHeight / 3;
  var pick = -1;
  var pickTop = -1e9;
  for (var i = 0; i < nodes.length; i++) {
    var top = nodes[i].getBoundingClientRect().top;
    if (top <= mark + 4 && top >= pickTop) {
      pickTop = top;
      pick = parseInt(nodes[i].id.slice(3), 10);
    }
  }
  if (pick < 0) pick = parseInt(nodes[0].id.slice(3), 10);
  return pick;
})()"#;
        self.eval_preview(script, move |value| {
            let line = value.and_then(|value| {
                let line = value.to_int32();
                (line >= 0).then_some(line as u32)
            });
            done(line);
        });
    }

    fn scroll_preview_to_line(&self, line: u32, attempt: u8) {
        let ctx = self.clone();
        let script = format!(
            r#"(function(){{
  var nodes = document.querySelectorAll('[id^="twl"]');
  if (!nodes.length || window.innerHeight < 1) return 0;
  var want = {line};
  var pick = null;
  var pickLine = -1;
  for (var i = 0; i < nodes.length; i++) {{
    var n = parseInt(nodes[i].id.slice(3), 10);
    if (n <= want && n >= pickLine) {{ pick = nodes[i]; pickLine = n; }}
  }}
  if (!pick) return 0;
  var scroller = document.scrollingElement || document.documentElement;
  var mark = window.innerHeight / 3;
  var y = pick.getBoundingClientRect().top + scroller.scrollTop;
  var dest = Math.max(0, Math.round(y - mark));
  scroller.scrollTop = dest;
  var landed = pick.getBoundingClientRect().top;
  if (Math.abs(landed - mark) > 6 && Math.abs(scroller.scrollTop - dest) < 2) return 0;
  return 1;
}})()"#
        );
        self.eval_preview(&script, move |value| {
            let placed = value.map(|value| value.to_int32() == 1).unwrap_or(false);
            if placed || attempt >= 20 {
                return;
            }
            glib::timeout_add_local_once(Duration::from_millis(50), move || {
                ctx.scroll_preview_to_line(line, attempt + 1);
            });
        });
    }

    fn eval_preview(
        &self,
        script: &str,
        done: impl FnOnce(Option<webkit6::javascriptcore::Value>) + 'static,
    ) {
        let preview = self.widgets.preview.clone();
        let was_on = WebViewExt::settings(&preview)
            .map(|settings| settings.enables_javascript())
            .unwrap_or(false);
        if let Some(settings) = WebViewExt::settings(&preview) {
            settings.set_enable_javascript(true);
        }
        preview.clone().evaluate_javascript(
            script,
            None,
            None,
            None::<&gio::Cancellable>,
            move |result| {
                if let Some(settings) = WebViewExt::settings(&preview) {
                    settings.set_enable_javascript(was_on);
                }
                done(result.ok());
            },
        );
    }

    fn apply_view_mode(&self) {
        self.refresh_nav_path();
        if self.state.borrow().browsing {
            self.widgets.content_stack.set_visible_child_name("folder");
            self.widgets
                .mode_button
                .set_icon_name("document-edit-symbolic");
            self.widgets
                .mode_button
                .set_tooltip_text(Some("Вернуться к файлу"));
            return;
        }
        let path = self.focused_path();
        if path.as_deref().is_some_and(kind::is_view_only) {
            let page = match path.as_deref().map(kind::shown) {
                Some(kind::Shown::Picture) => "picture",
                _ => "preview",
            };
            self.widgets.content_stack.set_visible_child_name(page);
            self.widgets
                .mode_button
                .set_icon_name("view-reveal-symbolic");
            self.widgets
                .mode_button
                .set_tooltip_text(Some("Только просмотр"));
            return;
        }
        let mode = self.state.borrow().view_mode;
        let show_preview = mode == ViewMode::Preview
            && path.as_ref().is_some_and(|path| {
                is_markdown(path)
                    || matches!(kind::shown(path), kind::Shown::Html | kind::Shown::Svg)
            });
        let editable = mode == ViewMode::Edit;
        for view in &self.widgets.views {
            view.set_editable(editable);
        }
        if show_preview {
            self.widgets.content_stack.set_visible_child_name("preview");
        } else {
            self.widgets.content_stack.set_visible_child_name("edit");
        }
        if mode == ViewMode::Preview {
            self.widgets
                .mode_button
                .set_icon_name("document-edit-symbolic");
            self.widgets
                .mode_button
                .set_tooltip_text(Some("Редактировать"));
        } else {
            self.widgets
                .mode_button
                .set_icon_name("view-reveal-symbolic");
            self.widgets.mode_button.set_tooltip_text(Some("Просмотр"));
        }
    }

    fn show_catalog_menu(&self, anchor: &gtk4::Button) {
        let popover = gtk4::Popover::new();
        popover.add_css_class("context-menu");
        popover.set_parent(anchor);
        popover.set_has_arrow(true);
        let menu = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        let disk = menu_row("В проводнике");
        let popover_close = popover.clone();
        let ctx = self.clone();
        disk.connect_clicked(move |_| {
            ctx.reveal_focused(TreePlace::Disk);
            popover_close.popdown();
        });
        menu.append(&disk);
        let workspace = menu_row("В пространстве");
        let popover_close = popover.clone();
        let ctx = self.clone();
        workspace.connect_clicked(move |_| {
            ctx.reveal_focused(TreePlace::Workspace);
            popover_close.popdown();
        });
        menu.append(&workspace);
        let contents = menu_row("Содержимое каталога");
        let popover_close = popover.clone();
        let ctx = self.clone();
        contents.connect_clicked(move |_| {
            ctx.browse_focused();
            popover_close.popdown();
        });
        menu.append(&contents);
        popover.set_child(Some(&menu));
        popover.connect_closed(|popover| {
            popover.unparent();
        });
        popover.popup();
    }

    fn refresh_open_list(&self) {
        clear_list(&self.widgets.open_list);
        let files = self.state.borrow().session.open_files.clone();
        let focused = self.focused_path();
        if files.is_empty() {
            self.widgets
                .open_list
                .append(&dim_row("Нет открытых файлов"));
        }
        for path in files {
            let emphasized = focused.as_ref() == Some(&path);
            self.widgets
                .open_list
                .append(&self.file_row(&path, emphasized, true));
        }
        self.refresh_pinned_list();
        self.refresh_history();
    }

    fn refresh_pinned_list(&self) {
        clear_list(&self.widgets.pinned_list);
        let files = self.state.borrow().session.pinned_files.clone();
        if files.is_empty() {
            self.widgets
                .pinned_list
                .append(&dim_row("Нет закреплённых файлов"));
            return;
        }
        let focused = self.focused_path();
        for path in files {
            let emphasized = focused.as_ref() == Some(&path);
            self.widgets
                .pinned_list
                .append(&self.file_row(&path, emphasized, false));
        }
    }

    fn refresh_history(&self) {
        clear_list(&self.widgets.history_list);
        let files = self.state.borrow().session.recent_files.clone();
        if files.is_empty() {
            self.widgets.history_list.append(&dim_row("История пуста"));
            return;
        }
        let focused = self.focused_path();
        for path in files {
            let emphasized = focused.as_ref() == Some(&path);
            self.widgets
                .history_list
                .append(&self.file_row(&path, emphasized, false));
        }
    }

    fn file_row(&self, path: &Path, emphasized: bool, closable: bool) -> gtk4::Box {
        let pinned = self
            .state
            .borrow()
            .session
            .pinned_files
            .iter()
            .any(|item| item == path);
        let dirty = self
            .state
            .borrow()
            .buffers
            .get(path)
            .is_some_and(|entry| entry.dirty);
        let name = file_label(path);
        let title = if dirty { format!("● {name}") } else { name };
        let label = gtk4::Label::new(Some(&title));
        label.set_halign(gtk4::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
        label.set_tooltip_text(Some(&path.display().to_string()));
        if emphasized {
            label.add_css_class("heading");
        }
        let pin = gtk4::Button::from_icon_name("view-pin-symbolic");
        pin.add_css_class("flat");
        pin.add_css_class("row-action");
        if pinned {
            pin.add_css_class("suggested-action");
            pin.set_tooltip_text(Some("Открепить файл"));
        } else {
            pin.set_tooltip_text(Some("Закрепить файл"));
        }
        let ctx = self.clone();
        let watch = path.to_path_buf();
        pin.connect_clicked(move |_| ctx.toggle_pin_file(&watch));
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        row.add_css_class("file-row");
        row.append(&file_icon(path));
        row.append(&label);
        row.append(&pin);
        if closable {
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.add_css_class("flat");
            close.add_css_class("row-action");
            close.set_tooltip_text(Some("Закрыть файл"));
            let ctx = self.clone();
            let watch = path.to_path_buf();
            close.connect_clicked(move |_| ctx.close_file(&watch));
            row.append(&close);
        }
        row
    }

    fn toggle_pin_file(&self, path: &Path) {
        if !path.is_file() {
            self.set_status("Закрепить можно только файл");
            return;
        }
        let pinned = {
            let mut state = self.state.borrow_mut();
            places::toggle_pin(&mut state.session.pinned_files, path.to_path_buf())
        };
        self.refresh_open_list();
        self.update_pin_button();
        self.save_session();
        self.set_status(if pinned {
            "Файл закреплён"
        } else {
            "Файл откреплён"
        });
    }

    fn update_pin_button(&self) {
        let path = self.focused_path();
        let pinned = path.as_ref().is_some_and(|path| {
            self.state
                .borrow()
                .session
                .pinned_files
                .iter()
                .any(|item| item == path)
        });
        self.widgets.pin_file_button.set_sensitive(path.is_some());
        if pinned {
            self.widgets
                .pin_file_button
                .add_css_class("suggested-action");
            self.widgets
                .pin_file_button
                .set_tooltip_text(Some("Открепить открытый файл"));
        } else {
            self.widgets
                .pin_file_button
                .remove_css_class("suggested-action");
            self.widgets
                .pin_file_button
                .set_tooltip_text(Some("Закрепить открытый файл"));
        }
    }

    fn jump_heading(&self, index: i32) {
        if index < 0 {
            return;
        }
        let line = self
            .state
            .borrow()
            .headings
            .get(index as usize)
            .map(|heading| heading.line);
        let Some(line) = line else {
            return;
        };
        let Some(path) = self.focused_path() else {
            return;
        };
        self.go(path, Some(line as u32));
    }

    fn refresh_outline(&self) {
        let headings = self.outline_headings();
        fill_outline(&self.widgets.outline_list, &headings);
        self.state.borrow_mut().headings = headings;
    }

    fn refresh_file_facts(&self) {
        clear_box(&self.widgets.file_facts);
        let Some(path) = self.focused_path() else {
            return;
        };
        let text = self.buffer_text(&path);
        let dirty = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.dirty);
        for (title, value) in about_rows(&path, text.as_deref(), dirty) {
            self.widgets.file_facts.append(&fact_row(title, &value));
        }
    }

    fn outline_headings(&self) -> Vec<markdown::Heading> {
        let Some(path) = self.focused_path() else {
            return Vec::new();
        };
        if !is_markdown(&path) {
            return Vec::new();
        }
        let Some(text) = self.buffer_text(&path) else {
            return Vec::new();
        };
        markdown::headings(&text)
    }

    fn schedule_preview(&self) {
        let ctx = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(PREVIEW_MS), move || {
            ctx.refresh_preview_now()
        });
    }

    fn refresh_preview_now(&self) {
        self.sync_open_tags();
        self.refresh_tag_chips();
        let Some(path) = self.focused_path() else {
            self.clear_picture();
            self.widgets
                .preview
                .load_html("<p>Файл не открыт.</p>", None);
            return;
        };
        match kind::shown(&path) {
            kind::Shown::Picture => {
                self.widgets
                    .picture
                    .set_file(Some(&gio::File::for_path(&path)));
                self.blank_preview();
                return;
            }
            kind::Shown::WebImage | kind::Shown::Pdf => {
                self.clear_picture();
                self.arm_preview(matches!(kind::shown(&path), kind::Shown::Pdf));
                self.widgets
                    .preview
                    .load_uri(&gio::File::for_path(&path).uri());
                return;
            }
            kind::Shown::Audio | kind::Shown::Video => {
                self.clear_picture();
                self.arm_preview(false);
                self.widgets.preview.load_html(
                    &media_page(
                        &gio::File::for_path(&path).uri(),
                        matches!(kind::shown(&path), kind::Shown::Video),
                    ),
                    Some(&gio::File::for_path(&path).uri()),
                );
                return;
            }
            kind::Shown::Svg => {
                self.clear_picture();
                self.arm_preview(false);
                let Some(text) = self.buffer_text(&path) else {
                    return;
                };
                let base = path.parent().unwrap_or_else(|| Path::new("/"));
                self.widgets.preview.load_bytes(
                    &glib::Bytes::from(text.as_bytes()),
                    Some("image/svg+xml"),
                    Some("UTF-8"),
                    Some(&gio::File::for_path(base).uri()),
                );
                return;
            }
            kind::Shown::Html => {
                self.clear_picture();
                self.arm_preview(false);
                let Some(text) = self.buffer_text(&path) else {
                    return;
                };
                let base = path.parent().unwrap_or_else(|| Path::new("/"));
                self.widgets
                    .preview
                    .load_html(&text, Some(&gio::File::for_path(base).uri()));
                return;
            }
            kind::Shown::Text => {}
        }
        self.clear_picture();
        if !is_markdown(&path) {
            self.widgets
                .preview
                .load_html(
                    "<p>Предпросмотр доступен для Markdown, HTML и SVG.</p>",
                    None,
                );
            return;
        }
        let Some(text) = self.buffer_text(&path) else {
            return;
        };
        let (pin, files) = {
            let state = self.state.borrow();
            (self.pin_path(), state.index.files.clone())
        };
        let html = markdown::render_html(
            &text,
            &path,
            pin.as_deref(),
            &files,
            &shell_link,
            "twmissing:",
        );
        self.arm_preview(false);
        self.widgets.preview.load_html(&html, None);
        self.refresh_backlinks();
        self.relayout_graph();
    }

    fn arm_preview(&self, javascript: bool) {
        let Some(settings) = WebViewExt::settings(&self.widgets.preview) else {
            return;
        };
        settings.set_enable_javascript(javascript);
        settings.set_enable_javascript_markup(javascript);
    }

    fn clear_picture(&self) {
        self.widgets.picture.set_file(None::<&gio::File>);
    }

    fn blank_preview(&self) {
        self.arm_preview(false);
        self.widgets.preview.load_html("<p></p>", None);
    }

    fn sync_open_tags(&self) {
        let Some(pin) = self.pin_path() else {
            return;
        };
        let paths: Vec<PathBuf> = self
            .state
            .borrow()
            .buffers
            .keys()
            .filter(|path| path.starts_with(&pin) && is_markdown(path))
            .cloned()
            .collect();
        let tagged: Vec<(PathBuf, Vec<String>)> = paths
            .into_iter()
            .filter_map(|path| {
                let text = self.buffer_text(&path)?;
                Some((path, markdown::extract_tags(&text)))
            })
            .collect();
        let mut state = self.state.borrow_mut();
        for (path, tags) in tagged {
            state.index.set_file_tags(&path, tags);
        }
    }

    fn sync_open_links(&self) {
        let Some(active) = self.pin_path() else {
            return;
        };
        let wiki_files = self.state.borrow().index.files.clone();
        let paths: Vec<PathBuf> = self
            .state
            .borrow()
            .buffers
            .keys()
            .filter(|path| is_markdown(path) && index::is_within(&active, path))
            .cloned()
            .collect();
        let linked: Vec<(PathBuf, Vec<PathBuf>)> = paths
            .into_iter()
            .filter_map(|path| {
                let text = self.buffer_text(&path)?;
                Some((
                    path.clone(),
                    index::outgoing_targets(&text, &path, &active, &wiki_files),
                ))
            })
            .collect();
        let mut state = self.state.borrow_mut();
        for (path, targets) in linked {
            state.index.set_file_outgoing(&path, targets);
        }
    }

    fn outgoing_of_note(&self, path: &Path) -> Vec<PathBuf> {
        let Some(active) = self.pin_path() else {
            return Vec::new();
        };
        let cached = self.state.borrow().index.outgoing_of(path).to_vec();
        if !is_markdown(path) {
            return cached;
        }
        let text = match self.buffer_text(path) {
            Some(text) => text,
            None => match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(_) => return cached,
            },
        };
        let pins = self.state.borrow().session.pins.clone();
        let scope = index::link_scope(path, &active, &pins).to_path_buf();
        let wiki_files = if scope == active {
            self.state.borrow().index.files.clone()
        } else if text.contains("[[") {
            self.state
                .borrow()
                .scope_graph
                .as_ref()
                .filter(|(root, _)| root == &scope)
                .map(|(_, graph)| graph.files.clone())
                .unwrap_or_else(|| index::markdown_in(&scope))
        } else {
            Vec::new()
        };
        index::outgoing_targets(&text, path, &scope, &wiki_files)
    }

    /// Входящие — обратные рёбра того же графа, из которого взяты исходящие.
    /// Для файла вне активного пина граф строится по его запиненному каталогу.
    fn incoming_of_note(&self, path: &Path) -> Vec<PathBuf> {
        let Some(active) = self.pin_path() else {
            return Vec::new();
        };
        let pins = self.state.borrow().session.pins.clone();
        let scope = index::link_scope(path, &active, &pins).to_path_buf();
        if scope == active {
            return self.state.borrow().index.incoming_edges(path);
        }
        if !self.scope_graph_ready(&scope) {
            self.kick_scope_graph(&scope);
            return Vec::new();
        }
        self.overlay_scope_buffers(&scope);
        let edges = self
            .state
            .borrow()
            .scope_graph
            .as_ref()
            .filter(|(root, _)| root == &scope)
            .map(|(_, graph)| graph.incoming_edges(path))
            .unwrap_or_default();
        edges
    }

    fn scope_graph_ready(&self, scope: &Path) -> bool {
        self.state
            .borrow()
            .scope_graph
            .as_ref()
            .is_some_and(|(root, _)| root == scope)
    }

    fn kick_scope_graph(&self, scope: &Path) {
        {
            let state = self.state.borrow();
            if state.scope_building.as_deref() == Some(scope) {
                return;
            }
        }
        let scope = scope.to_path_buf();
        self.state.borrow_mut().scope_building = Some(scope.clone());
        self.set_status("Считаются входящие ссылки каталога");
        let (sender, receiver) = std::sync::mpsc::channel();
        let build_root = scope.clone();
        std::thread::spawn(move || {
            let _ = sender.send(index::build_index(&build_root));
        });
        let ctx = self.clone();
        glib::timeout_add_local(Duration::from_millis(150), move || {
            match receiver.try_recv() {
                Ok(built) => {
                    let mut state = ctx.state.borrow_mut();
                    if state.scope_building.as_deref() == Some(scope.as_path()) {
                        state.scope_building = None;
                        state.scope_graph = Some((scope.clone(), built));
                    }
                    drop(state);
                    ctx.refresh_backlinks();
                    ctx.relayout_graph();
                    ctx.set_status("Входящие ссылки каталога готовы");
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    ctx.state.borrow_mut().scope_building = None;
                    glib::ControlFlow::Break
                }
            }
        });
    }

    fn overlay_scope_buffers(&self, scope: &Path) {
        let wiki_files = self
            .state
            .borrow()
            .scope_graph
            .as_ref()
            .filter(|(root, _)| root == scope)
            .map(|(_, graph)| graph.files.clone())
            .unwrap_or_default();
        let paths: Vec<PathBuf> = self
            .state
            .borrow()
            .buffers
            .keys()
            .filter(|path| is_markdown(path) && index::is_within(scope, path))
            .cloned()
            .collect();
        let linked: Vec<(PathBuf, Vec<PathBuf>)> = paths
            .into_iter()
            .filter_map(|path| {
                let text = self.buffer_text(&path)?;
                Some((
                    path.clone(),
                    index::outgoing_targets(&text, &path, scope, &wiki_files),
                ))
            })
            .collect();
        let mut state = self.state.borrow_mut();
        let Some((_, graph)) = state.scope_graph.as_mut() else {
            return;
        };
        for (path, targets) in linked {
            graph.set_file_outgoing(&path, targets);
        }
    }

    fn refresh_tag_chips(&self) {
        while let Some(child) = self.widgets.tag_box.first_child() {
            self.widgets.tag_box.remove(&child);
        }
        let Some(path) = self.focused_path() else {
            self.widgets.tag_box.set_visible(false);
            return;
        };
        if !is_markdown(&path) {
            self.widgets.tag_box.set_visible(false);
            return;
        }
        let Some(text) = self.buffer_text(&path) else {
            self.widgets.tag_box.set_visible(false);
            return;
        };
        let tags = markdown::extract_tags(&text);
        if tags.is_empty() {
            self.widgets.tag_box.set_visible(false);
            return;
        }
        let active = self.state.borrow().tag_query.clone();
        self.widgets.tag_box.set_visible(true);
        for tag in tags {
            let button = gtk4::Button::with_label(&format!("#{tag}"));
            button.add_css_class("flat");
            button.add_css_class("tag-chip");
            if active.as_deref() == Some(tag.as_str()) {
                button.add_css_class("tag-chip-on");
            }
            button.set_tooltip_text(Some("Показать файлы с этим тегом"));
            let ctx = self.clone();
            let chosen = tag.clone();
            button.connect_clicked(move |_| ctx.show_tag(&chosen));
            self.widgets.tag_box.append(&button);
        }
    }

    fn show_tag(&self, raw: &str) {
        let Some(tag) = markdown::canonical_tag(raw) else {
            return;
        };
        if self.pin_path().is_none() {
            self.set_status("Теги ищутся внутри запиненного каталога");
            return;
        }
        self.sync_open_tags();
        self.state.borrow_mut().tag_query = Some(tag.clone());
        self.show_panel("backlinks");
        self.refresh_backlinks();
        self.refresh_tag_chips();
        let count = self.state.borrow().backlinks.len();
        self.set_status(&format!("Тег #{tag}: {count}"));
    }

    fn refresh_backlinks(&self) {
        self.sync_open_links();
        let tag_query = self.state.borrow().tag_query.clone();
        if let Some(tag) = tag_query {
            self.widgets.link_head.set_visible(true);
            self.widgets.link_clear.set_visible(true);
            self.widgets.backlink_list.set_visible(true);
            self.widgets.incoming_fold.set_visible(false);
            self.widgets.outgoing_fold.set_visible(false);
            self.widgets.link_caption.set_text(&format!("Тег #{tag}"));
            clear_list(&self.widgets.backlink_list);
            if self.pin_path().is_none() {
                note_row(
                    &self.widgets.backlink_list,
                    "Теги ищутся внутри запиненного каталога",
                );
                self.state.borrow_mut().backlinks.clear();
                return;
            }
            let files = self.state.borrow().index.files_with_tag(&tag).to_vec();
            if files.is_empty() {
                note_row(&self.widgets.backlink_list, "Упоминаний нет");
            } else {
                fill_path_rows(&self.widgets.backlink_list, &files);
            }
            self.state.borrow_mut().backlinks = files;
            return;
        }
        self.widgets.link_head.set_visible(false);
        self.widgets.backlink_list.set_visible(false);
        self.widgets.incoming_fold.set_visible(true);
        self.widgets.outgoing_fold.set_visible(true);
        let Some(path) = self.focused_path() else {
            self.widgets.incoming_fold.set_label(Some("Входящие"));
            self.widgets.outgoing_fold.set_label(Some("Исходящие"));
            self.fill_link_box(&self.widgets.incoming_list, &[], "Файл не открыт");
            self.fill_link_box(&self.widgets.outgoing_list, &[], "Файл не открыт");
            self.state.borrow_mut().backlinks.clear();
            return;
        };
        if self.pin_path().is_none() {
            self.widgets.incoming_fold.set_label(Some("Входящие"));
            self.widgets.outgoing_fold.set_label(Some("Исходящие"));
            self.fill_link_box(
                &self.widgets.incoming_list,
                &[],
                "Ссылки считаются внутри запиненного каталога",
            );
            self.fill_link_box(
                &self.widgets.outgoing_list,
                &[],
                "Ссылки считаются внутри запиненного каталога",
            );
            self.state.borrow_mut().backlinks.clear();
            return;
        }
        let incoming = self.incoming_of_note(&path);
        let outgoing = self.outgoing_of_note(&path);
        self.widgets
            .incoming_fold
            .set_label(Some(&format!("Входящие ({})", incoming.len())));
        self.widgets
            .outgoing_fold
            .set_label(Some(&format!("Исходящие ({})", outgoing.len())));
        let waiting = {
            let state = self.state.borrow();
            match (
                state.session.active_pin_path(),
                state.scope_building.clone(),
            ) {
                (Some(active), Some(building)) => {
                    index::link_scope(&path, active, &state.session.pins) == building.as_path()
                }
                _ => false,
            }
        };
        let incoming_empty = if incoming.is_empty() && waiting {
            "Считаются входящие ссылки"
        } else {
            "Входящих ссылок нет"
        };
        self.fill_link_box(&self.widgets.incoming_list, &incoming, incoming_empty);
        self.fill_link_box(
            &self.widgets.outgoing_list,
            &outgoing,
            "Исходящих ссылок нет",
        );
        self.state.borrow_mut().backlinks = incoming;
    }

    fn fill_link_box(&self, parent: &gtk4::Box, files: &[PathBuf], empty: &str) {
        clear_box(parent);
        if files.is_empty() {
            let label = gtk4::Label::new(Some(empty));
            label.set_halign(gtk4::Align::Start);
            label.set_margin_start(8);
            label.set_margin_bottom(4);
            label.add_css_class("dim-label");
            parent.append(&label);
            return;
        }
        for path in files {
            let button = gtk4::Button::new();
            button.add_css_class("flat");
            button.add_css_class("link-row");
            let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            line.add_css_class("file-row");
            line.set_can_target(false);
            line.append(&file_icon(path));
            let label = gtk4::Label::new(Some(&file_label(path)));
            label.set_halign(gtk4::Align::Start);
            label.set_hexpand(true);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            line.append(&label);
            button.set_hexpand(true);
            button.set_child(Some(&line));
            button.set_tooltip_text(Some(&path.display().to_string()));
            let ctx = self.clone();
            let target = path.clone();
            button.connect_clicked(move |_| ctx.go(target.clone(), None));
            parent.append(&button);
        }
    }

    fn relayout_graph(&self) {
        let width = self.widgets.graph.width().max(280);
        let height = self.widgets.graph.height().max(360);
        let current = self.focused_path();
        let layout = {
            let state = self.state.borrow();
            let foreign = current.as_ref().and_then(|path| {
                let active = state.session.active_pin_path()?;
                let scope = index::link_scope(path, active, &state.session.pins);
                if scope == active {
                    None
                } else {
                    Some(scope.to_path_buf())
                }
            });
            let graph_index = foreign
                .as_ref()
                .and_then(|scope| {
                    state
                        .scope_graph
                        .as_ref()
                        .filter(|(root, _)| root == scope)
                        .map(|(_, graph)| graph)
                })
                .unwrap_or(&state.index);
            graph::layout(
                graph_index,
                current.as_deref(),
                f64::from(width),
                f64::from(height),
            )
        };
        self.state.borrow_mut().graph = layout;
        self.widgets.graph.queue_draw();
    }

    fn refresh_links(&self) {
        self.refresh_preview_now();
        self.refresh_backlinks();
        self.relayout_graph();
    }

    fn schedule_search(&self) {
        let ctx = self.clone();
        glib::timeout_add_local_once(Duration::from_millis(250), move || ctx.run_search());
    }

    fn run_search(&self) {
        let query = self.widgets.search_entry.text().to_string();
        let Some(pin) = self.pin_path() else {
            clear_list(&self.widgets.search_list);
            self.state.borrow_mut().search_hits.clear();
            self.set_status("Поиск доступен внутри запиненного каталога");
            return;
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let hits = index::search_pin(&pin, &query, 200);
            let _ = sender.send((query, hits));
        });
        let ctx = self.clone();
        glib::timeout_add_local(Duration::from_millis(150), move || {
            match receiver.try_recv() {
                Ok((query, hits)) => {
                    if ctx.widgets.search_entry.text().as_str() == query {
                        ctx.show_hits(hits);
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    fn show_hits(&self, hits: Vec<index::SearchHit>) {
        clear_list(&self.widgets.search_list);
        if hits.is_empty() {
            let label = gtk4::Label::new(Some("Ничего не найдено"));
            label.set_halign(gtk4::Align::Start);
            self.widgets.search_list.append(&label);
        }
        for hit in &hits {
            let text = format!(
                "{}:{}  {}",
                file_label(&hit.path),
                hit.line + 1,
                hit.preview
            );
            let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
            row.add_css_class("file-row");
            row.set_can_target(false);
            row.append(&file_icon(&hit.path));
            let label = gtk4::Label::new(Some(&text));
            label.set_halign(gtk4::Align::Start);
            label.set_hexpand(true);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            row.append(&label);
            self.widgets.search_list.append(&row);
        }
        self.set_status(&format!("Найдено: {}", hits.len()));
        self.state.borrow_mut().search_hits = hits;
    }

    fn reindex(&self) {
        let Some(pin) = self.pin_path() else {
            self.state.borrow_mut().index = index::LinkIndex::default();
            self.refresh_links();
            return;
        };
        let gen = {
            let mut state = self.state.borrow_mut();
            state.index_gen = state.index_gen.wrapping_add(1);
            state.index_gen
        };
        self.set_status("Строится индекс пространства");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(index::build_index(&pin));
        });
        let ctx = self.clone();
        glib::timeout_add_local(Duration::from_millis(150), move || {
            match receiver.try_recv() {
                Ok(built) => {
                    if ctx.state.borrow().index_gen == gen {
                        let truncated = built.truncated;
                        ctx.state.borrow_mut().index = built;
                        ctx.refresh_links();
                        if truncated {
                            ctx.set_status("Индекс обрезан: слишком много Markdown-файлов");
                        } else {
                            ctx.set_status("Индекс пространства готов");
                        }
                    }
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    fn handle_uri(&self, uri: &glib::GString, decision: &webkit6::PolicyDecision) -> bool {
        if let Some(rest) = uri.strip_prefix("twopen:") {
            decision.ignore();
            let (encoded, line) = match rest.split_once('?') {
                Some((path, query)) => {
                    let line = query
                        .strip_prefix("line=")
                        .and_then(|value| value.parse::<u32>().ok());
                    (path, line)
                }
                None => (rest, None),
            };
            if let Some(path) = markdown::decode_path(encoded) {
                self.go(PathBuf::from(path), line);
            }
            return true;
        }
        if let Some(rest) = uri.strip_prefix("twpick:") {
            decision.ignore();
            if let Some(name) = markdown::decode_path(rest) {
                self.pick_wiki(&name);
            }
            return true;
        }
        if let Some(rest) = uri.strip_prefix("twtag:") {
            decision.ignore();
            if let Some(tag) = markdown::decode_path(rest) {
                self.show_tag(&tag);
            }
            return true;
        }
        if let Some(rest) = uri.strip_prefix("twmissing:") {
            decision.ignore();
            let name = markdown::decode_path(rest).unwrap_or_else(|| rest.to_string());
            self.set_status(&format!("Файл «{name}» не найден в пространстве"));
            return true;
        }
        if uri.starts_with("http:") || uri.starts_with("https:") || uri.starts_with("file:") {
            decision.ignore();
            self.set_status("Внешние ссылки не открываются");
            return true;
        }
        false
    }

    fn pick_wiki(&self, name: &str) {
        let Some(pin) = self.pin_path() else {
            return;
        };
        let files = self.state.borrow().index.files.clone();
        let matches = markdown::resolve_wiki(name, &pin, &files);
        match matches.as_slice() {
            [only] => self.go(only.clone(), None),
            [] => self.set_status(&format!("Файл «{name}» не найден в пространстве")),
            many => self.show_pick(name, many),
        }
    }

    fn show_pick(&self, name: &str, matches: &[PathBuf]) {
        let dialog = gtk4::Window::builder()
            .transient_for(&self.widgets.window)
            .modal(true)
            .title(format!("Несколько файлов «{name}»"))
            .default_width(480)
            .default_height(320)
            .build();
        let list = gtk4::ListBox::new();
        for path in matches {
            let label = gtk4::Label::new(Some(&path.display().to_string()));
            label.set_halign(gtk4::Align::Start);
            list.append(&label);
        }
        let ctx = self.clone();
        let paths = matches.to_vec();
        let dialog_close = dialog.clone();
        list.connect_row_activated(move |_, row| {
            let index = row.index();
            if let Some(path) = paths.get(index as usize).cloned() {
                ctx.go(path, None);
                dialog_close.close();
            }
        });
        dialog.set_child(Some(&scrolled(&list)));
        dialog.present();
    }

    fn show_panel(&self, id: &str) {
        let Some(spec) = seams::panels().iter().find(|panel| panel.id == id) else {
            return;
        };
        let notebook = match spec.side {
            seams::PanelSide::Left => &self.widgets.left_notebook,
            seams::PanelSide::Right => &self.widgets.right_notebook,
        };
        let Some(index) = notebook_index_of(notebook, id) else {
            return;
        };
        notebook.set_current_page(Some(index));
        if id == "search" {
            self.widgets.search_entry.grab_focus();
        }
    }

    fn show_find(&self) {
        if self
            .focused_path()
            .is_some_and(|path| kind::is_view_only(&path))
        {
            self.set_status("В этом файле нечего искать: он только для просмотра");
            return;
        }
        self.change_view_mode(ViewMode::Edit);
        self.widgets.find_bar.set_visible(true);
        self.widgets.find_entry.grab_focus();
    }

    fn hide_find(&self) {
        self.widgets.find_bar.set_visible(false);
        self.focused_view().grab_focus();
    }

    fn find_move(&self, forward: bool) {
        let query = self.widgets.find_entry.text().to_string();
        if query.is_empty() {
            return;
        }
        let view = self.focused_view();
        let buffer = view.buffer();
        let flags = gtk4::TextSearchFlags::CASE_INSENSITIVE | gtk4::TextSearchFlags::TEXT_ONLY;
        let (start, end) = buffer.selection_bounds().unwrap_or_else(|| {
            let iter = buffer.iter_at_offset(buffer.cursor_position().max(0));
            (iter.clone(), iter)
        });
        let found = if forward {
            end.forward_search(&query, flags, None)
                .or_else(|| buffer.start_iter().forward_search(&query, flags, None))
        } else {
            start
                .backward_search(&query, flags, None)
                .or_else(|| buffer.end_iter().backward_search(&query, flags, None))
        };
        if let Some((mut match_start, match_end)) = found {
            buffer.select_range(&match_start, &match_end);
            view.scroll_to_iter(&mut match_start, 0.1, true, 0.0, 0.3);
            view.grab_focus();
        } else {
            self.set_status("Совпадений нет");
        }
    }

    fn quick_open(&self) {
        let Some(pin) = self.pin_path() else {
            self.set_status("Быстрый переход работает внутри запиненного пространства");
            return;
        };
        let files = index::list_text_files(&pin);
        let dialog = gtk4::Window::builder()
            .transient_for(&self.widgets.window)
            .modal(true)
            .title("Перейти к файлу")
            .default_width(560)
            .default_height(420)
            .build();
        let entry = gtk4::SearchEntry::new();
        entry.set_placeholder_text(Some("Имя или часть пути"));
        let list = gtk4::ListBox::new();
        list.add_css_class("navigation-sidebar");
        let shown = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
        let refill: Rc<dyn Fn()> = Rc::new({
            let files = files.clone();
            let list = list.clone();
            let shown = shown.clone();
            let entry = entry.clone();
            move || {
                clear_list(&list);
                let query = entry.text().to_lowercase();
                let mut next = Vec::new();
                for path in &files {
                    let text = path.display().to_string().to_lowercase();
                    let name = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if !query.is_empty() && !name.contains(&query) && !text.contains(&query) {
                        continue;
                    }
                    let label = gtk4::Label::new(Some(&path.display().to_string()));
                    label.set_halign(gtk4::Align::Start);
                    label.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
                    list.append(&label);
                    next.push(path.clone());
                    if next.len() == 80 {
                        break;
                    }
                }
                if next.is_empty() {
                    list.append(&dim_row("Ничего не найдено"));
                }
                *shown.borrow_mut() = next;
            }
        });
        refill();
        let refill_changed = refill.clone();
        entry.connect_search_changed(move |_| refill_changed());
        let ctx = self.clone();
        let shown_activate = shown.clone();
        let dialog_activate = dialog.clone();
        entry.connect_activate(move |_| {
            let path = shown_activate.borrow().first().cloned();
            if let Some(path) = path {
                ctx.go(path, None);
                dialog_activate.close();
            }
        });
        let ctx = self.clone();
        let dialog_row = dialog.clone();
        list.connect_row_activated(move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            let path = shown.borrow().get(index as usize).cloned();
            if let Some(path) = path {
                ctx.go(path, None);
                dialog_row.close();
            }
        });
        let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
        box_.set_margin_top(8);
        box_.set_margin_bottom(8);
        box_.set_margin_start(8);
        box_.set_margin_end(8);
        box_.append(&entry);
        box_.append(&scrolled(&list));
        dialog.set_child(Some(&box_));
        dialog.present();
        entry.grab_focus();
    }

    fn set_nav_pinned(&self, pinned: bool) {
        if self.state.borrow().session.nav_pinned == pinned {
            self.apply_nav_dock();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.session.nav_pinned = pinned;
            state.session.nav_open = pinned;
        }
        self.apply_nav_dock();
        self.save_session();
    }

    fn fold_nav(&self) {
        self.state.borrow_mut().session.nav_open = false;
        self.apply_nav_dock();
        self.save_session();
    }

    fn expand_nav(&self) {
        self.state.borrow_mut().session.nav_open = true;
        self.apply_nav_dock();
        self.save_session();
    }

    fn apply_nav_dock(&self) {
        let (open, pinned, width) = {
            let session = &self.state.borrow().session;
            (session.nav_open, session.nav_pinned, session.nav_width)
        };
        self.widgets.nav_pin.set_active(pinned);
        self.widgets.nav_rail_pin.set_active(pinned);
        self.widgets.nav_fold.set_sensitive(true);
        self.widgets.left_notebook.set_visible(open);
        self.widgets.nav_rail.set_visible(false);
        self.widgets.activity_bar.set_visible(true);
        // Минимальная ширина — это width-request, а не текущая.
        // shrink выключен: ниже минимума панель не сдвигается за край окна.
        self.widgets.main_paned.set_shrink_start_child(false);
        self.widgets.main_paned.set_resize_start_child(open);
        if let Some(shell) = self
            .widgets
            .left_notebook
            .parent()
            .and_then(|column| column.parent())
        {
            shell.set_width_request(if open {
                session::NAV_MIN_WIDTH + session::ACTIVITY_WIDTH
            } else {
                session::ACTIVITY_WIDTH
            });
        }
        if open {
            self.widgets
                .left_notebook
                .set_width_request(session::NAV_MIN_WIDTH);
            self.place_paned(
                &self.widgets.main_paned,
                width.clamp(
                    session::NAV_MIN_WIDTH + session::ACTIVITY_WIDTH,
                    session::NAV_MAX_WIDTH,
                ),
            );
        } else {
            self.widgets.left_notebook.set_width_request(-1);
            self.place_paned(&self.widgets.main_paned, session::ACTIVITY_WIDTH);
        }
        self.sync_activity();
        self.refresh_nav_path();
    }

    fn sync_activity(&self) {
        let id = self.state.borrow().session.left_panel.clone();
        for button in &self.widgets.activity_buttons {
            if button.widget_name().as_str() == id {
                button.add_css_class("activity-on");
            } else {
                button.remove_css_class("activity-on");
            }
        }
    }

    fn watch_tree_directory(&self, list: &gtk4::ListView) {
        let ctx = self.clone();
        let click = gtk4::GestureClick::new();
        click.set_button(gtk4::gdk::BUTTON_PRIMARY);
        click.set_propagation_phase(gtk4::PropagationPhase::Bubble);
        click.connect_released(move |gesture, _, _, _| {
            let Some(list) = gesture
                .widget()
                .and_then(|widget| widget.downcast::<gtk4::ListView>().ok())
            else {
                return;
            };
            ctx.remember_tree_directory(&list);
        });
        list.add_controller(click);
        let ctx = self.clone();
        list.connect_activate(move |list, _| ctx.remember_tree_directory(list));
    }

    fn remember_tree_directory(&self, list: &gtk4::ListView) {
        let Some(dir) = tree_directory(list) else {
            return;
        };
        self.state.borrow_mut().nav_dir = Some(dir);
        self.refresh_nav_path();
    }

    /// Каталог левой панели: выбранная строка дерева, иначе каталог открытого файла.
    fn refresh_nav_path(&self) {
        let (open, panel) = {
            let session = &self.state.borrow().session;
            (session.nav_open, session.left_panel.clone())
        };
        if !open {
            self.widgets.nav_path.set_visible(false);
            return;
        }
        let (text, tip) = match panel.as_str() {
            "disk" => {
                let text = self.disk_directory().display().to_string();
                (text.clone(), Some(text))
            }
            "workspace" => match self.workspace_directory() {
                Some(path) => {
                    let text = path.display().to_string();
                    (text.clone(), Some(text))
                }
                None => ("Пространство не выбрано".to_string(), None),
            },
            "notes" => {
                let text = text_workspace::notes::notes_dir().display().to_string();
                (text.clone(), Some(text))
            }
            _ => {
                self.widgets.nav_path.set_visible(false);
                return;
            }
        };
        self.widgets.nav_path.set_text(&text);
        self.widgets.nav_path.set_tooltip_text(tip.as_deref());
        self.widgets.nav_path.set_visible(true);
    }

    fn disk_directory(&self) -> PathBuf {
        if let Some(dir) = self.state.borrow().nav_dir.clone() {
            return dir;
        }
        if self.state.borrow().browsing {
            if let Some(dir) = self.widgets.folder_dir.file().and_then(|file| file.path()) {
                return dir;
            }
        }
        if let Some(dir) = self
            .focused_path()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        {
            if !dir.as_os_str().is_empty() {
                return dir;
            }
        }
        PathBuf::from("/")
    }

    fn workspace_directory(&self) -> Option<PathBuf> {
        let pin = self.pin_path()?;
        if let Some(dir) = self.state.borrow().nav_dir.clone() {
            if dir.starts_with(&pin) {
                return Some(dir);
            }
        }
        if let Some(dir) = self
            .focused_path()
            .and_then(|path| path.parent().map(Path::to_path_buf))
        {
            if dir.starts_with(&pin) {
                return Some(dir);
            }
        }
        Some(pin)
    }

    fn nav_floor(&self, paned: &gtk4::Paned) -> i32 {
        let content = paned
            .start_child()
            .map(|child| child.measure(gtk4::Orientation::Horizontal, -1).0)
            .unwrap_or(session::NAV_MIN_WIDTH);
        content.clamp(session::NAV_MIN_WIDTH, session::NAV_MAX_WIDTH)
    }

    fn info_floor(&self, paned: &gtk4::Paned) -> i32 {
        let content = paned
            .end_child()
            .map(|child| child.measure(gtk4::Orientation::Horizontal, -1).0)
            .unwrap_or(session::INFO_MIN_WIDTH);
        content.clamp(session::INFO_MIN_WIDTH, 480)
    }

    fn set_info_pinned(&self, pinned: bool) {
        if self.state.borrow().session.info_pinned == pinned {
            self.apply_info_dock();
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.session.info_pinned = pinned;
            state.session.info_open = pinned;
        }
        self.apply_info_dock();
        self.save_session();
    }

    fn fold_info(&self) {
        self.state.borrow_mut().session.info_open = false;
        self.apply_info_dock();
        self.save_session();
    }

    fn expand_info(&self) {
        self.state.borrow_mut().session.info_open = true;
        self.apply_info_dock();
        self.save_session();
    }

    fn apply_info_dock(&self) {
        let (open, pinned, width) = {
            let session = &self.state.borrow().session;
            (session.info_open, session.info_pinned, session.info_width)
        };
        let end = if open {
            width.clamp(session::INFO_MIN_WIDTH, 480)
        } else {
            session::DOCK_RAIL_WIDTH
        };
        self.widgets.info_pin.set_active(pinned);
        self.widgets.info_rail_pin.set_active(pinned);
        self.widgets.info_fold.set_sensitive(true);
        self.widgets.doc_side.set_visible(open);
        self.widgets.info_rail.set_visible(!open);
        self.widgets.doc_side.set_hexpand(open);
        self.widgets.doc_side.set_halign(gtk4::Align::Fill);
        self.widgets.doc_side.set_width_request(if open {
            session::INFO_MIN_WIDTH
        } else {
            -1
        });
        if let Some(shell) = self.widgets.doc_side.parent() {
            shell.set_hexpand(open);
            shell.set_halign(gtk4::Align::Fill);
            shell.set_overflow(gtk4::Overflow::Hidden);
            shell.set_width_request(if open {
                session::INFO_MIN_WIDTH
            } else {
                end
            });
        }
        let paned = self.widgets.right_paned.clone();
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(open);
        paned.set_shrink_end_child(false);
        let total = paned.width();
        if total > end + 200 {
            self.place_paned(&paned, total - end);
        }
    }

    fn place_paned(&self, paned: &gtk4::Paned, position: i32) {
        self.state.borrow_mut().dock_adjust = true;
        paned.set_position(position);
        self.state.borrow_mut().dock_adjust = false;
    }

    fn autohide_docks(&self) {
        if self.state.borrow().loading {
            return;
        }
        let (hide_nav, hide_info) = {
            let session = &self.state.borrow().session;
            (
                session.nav_open && !session.nav_pinned,
                session.info_open && !session.info_pinned,
            )
        };
        if hide_nav {
            self.state.borrow_mut().session.nav_open = false;
            self.apply_nav_dock();
        }
        if hide_info {
            self.state.borrow_mut().session.info_open = false;
            self.apply_info_dock();
        }
        if hide_nav || hide_info {
            self.save_session();
        }
    }

    fn set_status(&self, text: &str) {
        let zoom = self.state.borrow().session.zoom_pt;
        self.widgets
            .status
            .set_text(&format!("{text}    масштаб {zoom} пт"));
    }

    fn save_session(&self) {
        if self.state.borrow().loading {
            return;
        }
        let left_order = notebook_order(&self.widgets.left_notebook);
        let right_order = notebook_order(&self.widgets.right_notebook);
        let session = {
            let mut state = self.state.borrow_mut();
            self.capture_carets(&mut state);
            state.session.view_mode = state.view_mode;
            let width = self.widgets.window.width();
            let height = self.widgets.window.height();
            if width >= session::MIN_WINDOW_WIDTH && height >= session::MIN_WINDOW_HEIGHT {
                state.session.window_width = width;
                state.session.window_height = height;
            }
            // Текущая страница блокнота меняется после сигнала switch-page.
            // Раздел уже записан обработчиком, здесь его нельзя перечитывать.
            if left_order.len() as u32 == self.widgets.left_notebook.n_pages() {
                state.session.left_order = left_order;
            }
            if right_order.len() as u32 == self.widgets.right_notebook.n_pages() {
                state.session.right_order = right_order;
            }
            state.session.clone()
        };
        if let Err(error) = session.save() {
            self.set_status(&format!("Не удалось сохранить сессию: {error}"));
        }
    }
}
