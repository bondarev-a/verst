use std::cell::RefCell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gio::prelude::ListModelExt;
use gtk4::prelude::*;
use gtk4::{gio, glib};
use sourceview5::prelude::ViewExt;

use text_workspace::{graph, kind, markdown, seams};

use super::{Ctx, MAX_OPEN_BYTES, TREE_ATTRS};

pub(super) const CHROME_CSS: &str = "
.pathbar {
    margin: 6px 12px 0 12px;
    padding: 4px 6px;
    border-radius: 12px;
    background-color: alpha(@theme_fg_color, 0.055);
}
.pathbar .path-label {
    font-family: monospace;
}
.pinbar {
    margin: 8px 12px 0 12px;
}
.findbar {
    margin: 8px 12px 0 12px;
}
.side-caption {
    margin: 10px 12px 2px 12px;
}
.links-page expander {
    margin: 4px 8px 0 8px;
}
.links-page list {
    background: transparent;
}
.links-page button.link-row {
    padding: 2px 8px;
    border-radius: 8px;
}
.doc-side {
    margin-right: 8px;
}
.file-facts {
    margin: 2px 10px 8px 10px;
}
.workspace-main {
    margin: 8px 10px 0 10px;
}
.statusbar {
    margin: 4px 14px 8px 14px;
}
notebook.sidebar-notebook > header {
    background: transparent;
}
notebook.sidebar-notebook > header > tabs > tab {
    border-radius: 8px;
    margin: 4px 2px;
    padding: 2px 10px;
    border-width: 0;
    background-image: none;
    box-shadow: none;
}
notebook.sidebar-notebook > header > tabs > tab:checked,
notebook.sidebar-notebook > header.top > tabs > tab:checked {
    box-shadow: none;
    background-color: alpha(@theme_fg_color, 0.14);
}
button.tag-chip {
    border-radius: 999px;
    padding: 1px 8px;
    min-height: 0;
}
button.tag-chip-on {
    background-color: alpha(@theme_selected_bg_color, 0.35);
}
listview > row {
    border-radius: 8px;
    margin: 1px 6px;
}
.explorer listview > row {
    margin: 0;
    border-radius: 0;
}
.explorer-bar {
    margin: 8px 12px 0 12px;
}
button.crumb {
    padding: 2px 8px;
    min-height: 0;
}
button.mode-toggle {
    border-radius: 999px;
    padding: 8px;
    background-color: alpha(@theme_bg_color, 0.9);
    box-shadow: 0 1px 3px alpha(black, 0.28), 0 8px 18px alpha(black, 0.14);
    opacity: 0.92;
}
button.mode-toggle:hover {
    opacity: 1;
}
headerbar {
    background: transparent;
    box-shadow: none;
    border-bottom: none;
}
menubar {
    background: transparent;
    box-shadow: none;
    padding: 2px 8px 4px 8px;
    border-bottom: 1px solid alpha(@theme_fg_color, 0.08);
}
menubar > item,
menubar item button {
    border-radius: 999px;
    padding: 4px 14px;
    margin: 0 2px;
    background: transparent;
    box-shadow: none;
    border: none;
}
menubar > item:hover,
menubar > item:selected,
menubar item button:hover,
menubar item button:checked {
    background-color: alpha(@theme_fg_color, 0.10);
}
popover.menu,
popover.context-menu {
    background-color: transparent;
    border-radius: 16px;
}
popover.menu > contents,
popover.context-menu > contents,
popover.menu.background > contents,
popover.context-menu.background > contents {
    padding: 6px;
    border-radius: 16px;
    background-image: none;
    background-color: @theme_bg_color;
    box-shadow:
        0 1px 2px alpha(black, 0.18),
        0 16px 40px alpha(black, 0.45),
        0 0 0 1px alpha(@theme_fg_color, 0.18);
}
popover.menu scrolledwindow,
popover.menu viewport,
popover.menu stack,
popover.context-menu scrolledwindow,
popover.context-menu viewport {
    background-color: transparent;
    background-image: none;
}
popover.menu modelbutton {
    border-radius: 10px;
    padding: 7px 12px;
    min-height: 32px;
    min-width: 14em;
}
popover.menu modelbutton:hover {
    background-color: alpha(@theme_fg_color, 0.08);
}
popover.menu modelbutton:active {
    background-color: alpha(@theme_selected_bg_color, 0.34);
}
popover.menu modelbutton label.accelerator {
    opacity: 0.55;
}
popover.menu separator {
    margin: 6px 10px;
    min-height: 1px;
    background-color: alpha(@theme_fg_color, 0.12);
}
popover.context-menu {
    min-width: 220px;
}
popover.context-menu button.menu-row {
    border-radius: 10px;
    padding: 8px 12px;
    margin: 1px 0;
    min-height: 0;
}
popover.context-menu button.menu-row:hover {
    background-color: alpha(@theme_fg_color, 0.08);
}
popover.context-menu button.menu-row:active {
    background-color: alpha(@theme_selected_bg_color, 0.34);
}
.file-facts {
    padding: 8px 10px;
    border-radius: 12px;
    background-color: alpha(@theme_fg_color, 0.045);
}
entry {
    border-radius: 10px;
}
window.prompt-dialog {
    border-radius: 18px;
}
window.prompt-dialog headerbar {
    border-radius: 18px 18px 0 0;
    border-bottom: none;
}
window.prompt-dialog entry {
    min-height: 34px;
    padding: 6px 12px;
    border-radius: 12px;
}
window.prompt-dialog button {
    border-radius: 12px;
    padding: 6px 16px;
    min-width: 88px;
}
";

pub(super) fn make_view(buffer: &sourceview5::Buffer) -> sourceview5::View {
    let view = sourceview5::View::with_buffer(buffer);
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_highlight_current_line(true);
    view.set_hexpand(true);
    view.set_vexpand(true);
    view
}

#[derive(Clone, Copy)]
pub(super) enum TreePlace {
    Disk,
    Workspace,
}

#[derive(Clone)]
pub(super) struct TreeActions {
    pub(super) open: Rc<dyn Fn(PathBuf)>,
    pub(super) pin: Rc<dyn Fn(PathBuf)>,
    pub(super) pin_file: Rc<dyn Fn(PathBuf)>,
    pub(super) is_pinned: Rc<dyn Fn(&Path) -> bool>,
    pub(super) notice: Rc<dyn Fn(String)>,
    pub(super) reveal_disk: Rc<dyn Fn(PathBuf)>,
    pub(super) reveal_workspace: Rc<dyn Fn(PathBuf)>,
    pub(super) browse: Rc<dyn Fn(PathBuf)>,
    pub(super) mkdir: Rc<dyn Fn(PathBuf)>,
    pub(super) mkfile: Rc<dyn Fn(PathBuf)>,
}

pub(super) struct RowClick {
    path: PathBuf,
    at: std::time::Instant,
}

#[derive(Clone, Copy)]
pub(super) enum FolderKey {
    Name,
    Date,
    Kind,
    Size,
}

pub(super) fn folder_columns(directory: &gtk4::DirectoryList) -> gtk4::ColumnView {
    let view = gtk4::ColumnView::new(None::<gtk4::SingleSelection>);
    let sorted = gtk4::SortListModel::new(Some(directory.clone()), view.sorter());
    let selection = gtk4::SingleSelection::new(Some(sorted));
    view.set_model(Some(&selection));
    view.set_single_click_activate(false);
    view.set_show_row_separators(true);
    view.set_enable_rubberband(true);
    view.set_vexpand(true);
    view.set_hexpand(true);
    view.append_column(&name_column());
    view.append_column(&text_column(
        "Изменён",
        gtk4::Align::Start,
        150,
        FolderKey::Date,
        modified_label,
    ));
    view.append_column(&text_column(
        "Тип",
        gtk4::Align::Start,
        120,
        FolderKey::Kind,
        kind_label,
    ));
    view.append_column(&text_column(
        "Размер",
        gtk4::Align::End,
        90,
        FolderKey::Size,
        size_label,
    ));
    view
}

pub(super) fn name_column() -> gtk4::ColumnViewColumn {
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        row.set_margin_start(6);
        row.set_margin_end(6);
        let image = gtk4::Image::new();
        image.set_pixel_size(16);
        let label = gtk4::Label::new(None);
        label.set_halign(gtk4::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        row.append(&image);
        row.append(&label);
        item.set_child(Some(&row));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let Some(row) = item
            .child()
            .and_then(|child| child.downcast::<gtk4::Box>().ok())
        else {
            return;
        };
        let Some(image) = row
            .first_child()
            .and_then(|child| child.downcast::<gtk4::Image>().ok())
        else {
            return;
        };
        let Some(label) = row
            .last_child()
            .and_then(|child| child.downcast::<gtk4::Label>().ok())
        else {
            return;
        };
        let Some(info) = item
            .item()
            .and_then(|item| item.downcast::<gio::FileInfo>().ok())
        else {
            return;
        };
        label.set_text(&info.display_name());
        if let Some(icon) = info.symbolic_icon() {
            image.set_from_gicon(&icon);
        } else if info.file_type() == gio::FileType::Directory {
            image.set_icon_name(Some("folder-symbolic"));
        } else {
            image.set_icon_name(Some("text-x-generic-symbolic"));
        }
    });
    let column = gtk4::ColumnViewColumn::new(Some("Имя"), Some(factory));
    column.set_expand(true);
    column.set_resizable(true);
    column.set_sorter(Some(&folder_sorter(FolderKey::Name)));
    column
}

pub(super) fn text_column(
    title: &str,
    align: gtk4::Align,
    width: i32,
    key: FolderKey,
    text: impl Fn(&gio::FileInfo) -> String + 'static,
) -> gtk4::ColumnViewColumn {
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let label = gtk4::Label::new(None);
        label.set_halign(align);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_margin_start(6);
        label.set_margin_end(8);
        item.set_child(Some(&label));
    });
    let text = Rc::new(text);
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let Some(label) = item
            .child()
            .and_then(|child| child.downcast::<gtk4::Label>().ok())
        else {
            return;
        };
        let Some(info) = item
            .item()
            .and_then(|item| item.downcast::<gio::FileInfo>().ok())
        else {
            return;
        };
        label.set_text(&text(&info));
    });
    let column = gtk4::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_resizable(true);
    column.set_fixed_width(width);
    column.set_sorter(Some(&folder_sorter(key)));
    column
}

pub(super) fn folder_sorter(key: FolderKey) -> gtk4::CustomSorter {
    gtk4::CustomSorter::new(move |left, right| {
        let Some(left) = left.downcast_ref::<gio::FileInfo>() else {
            return gtk4::Ordering::Equal;
        };
        let Some(right) = right.downcast_ref::<gio::FileInfo>() else {
            return gtk4::Ordering::Equal;
        };
        let left_dir = left.file_type() == gio::FileType::Directory;
        let right_dir = right.file_type() == gio::FileType::Directory;
        if left_dir != right_dir {
            return if left_dir {
                gtk4::Ordering::Smaller
            } else {
                gtk4::Ordering::Larger
            };
        }
        let order = match key {
            FolderKey::Name => left
                .display_name()
                .to_lowercase()
                .cmp(&right.display_name().to_lowercase()),
            FolderKey::Date => mtime(left).cmp(&mtime(right)),
            FolderKey::Kind => kind_label(left).cmp(&kind_label(right)),
            FolderKey::Size => left.size().cmp(&right.size()),
        };
        match order {
            std::cmp::Ordering::Less => gtk4::Ordering::Smaller,
            std::cmp::Ordering::Equal => gtk4::Ordering::Equal,
            std::cmp::Ordering::Greater => gtk4::Ordering::Larger,
        }
    })
}

pub(super) fn mtime(info: &gio::FileInfo) -> i64 {
    info.modification_date_time()
        .map(|time| time.to_unix())
        .unwrap_or(0)
}

pub(super) fn modified_label(info: &gio::FileInfo) -> String {
    let Some(time) = info.modification_date_time() else {
        return String::new();
    };
    let local = time.to_local().unwrap_or(time);
    local
        .format("%d.%m.%Y %H:%M")
        .map(|text| text.to_string())
        .unwrap_or_default()
}

pub(super) fn kind_label(info: &gio::FileInfo) -> String {
    match info.file_type() {
        gio::FileType::Directory => "Папка".to_string(),
        gio::FileType::SymbolicLink => "Ссылка".to_string(),
        gio::FileType::Special => "Устройство".to_string(),
        _ => match info
            .display_name()
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_uppercase())
            .filter(|ext| !ext.is_empty() && ext.len() <= 8)
        {
            Some(ext) => format!("файл {ext}"),
            None => "Файл".to_string(),
        },
    }
}

pub(super) fn size_label(info: &gio::FileInfo) -> String {
    if info.file_type() == gio::FileType::Directory || info.size() < 0 {
        return String::new();
    }
    format_size(info.size() as u64)
}

pub(super) fn format_size(size: u64) -> String {
    const UNITS: [&str; 4] = ["Б", "КБ", "МБ", "ГБ"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{size} {}", UNITS[0])
    } else if value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(super) fn file_tree(
    root: &Path,
    actions: TreeActions,
) -> (gtk4::ScrolledWindow, gtk4::ListView) {
    let file = gio::File::for_path(root);
    let model = sorted_dir(&file);
    let tree = gtk4::TreeListModel::new(model, false, false, |item| {
        let info = item.downcast_ref::<gio::FileInfo>()?;
        if info.file_type() != gio::FileType::Directory {
            return None;
        }
        let child = info
            .attribute_object("standard::file")?
            .downcast::<gio::File>()
            .ok()?;
        Some(sorted_dir(&child).upcast())
    });
    let selection = gtk4::SingleSelection::new(Some(tree));
    let last_click = Rc::new(RefCell::new(None::<RowClick>));
    let factory = tree_factory(actions.clone(), last_click);
    let list = gtk4::ListView::new(Some(selection), Some(factory));
    list.set_single_click_activate(false);
    list.set_vexpand(true);
    let open = actions.open.clone();
    list.connect_activate(move |list, position| {
        let Some(path) = list
            .model()
            .and_then(|model| model.item(position))
            .and_then(|item| item.downcast::<gtk4::TreeListRow>().ok())
            .as_ref()
            .and_then(row_path)
        else {
            return;
        };
        if path.is_file() {
            open(path);
        }
    });
    (scrolled(&list), list)
}

pub(super) fn sorted_dir(file: &gio::File) -> gtk4::SortListModel {
    let directory = gtk4::DirectoryList::new(Some(TREE_ATTRS), Some(file));
    directory.set_monitored(true);
    let sorter = gtk4::CustomSorter::new(|left, right| {
        let Some(left) = left.downcast_ref::<gio::FileInfo>() else {
            return gtk4::Ordering::Equal;
        };
        let Some(right) = right.downcast_ref::<gio::FileInfo>() else {
            return gtk4::Ordering::Equal;
        };
        let left_dir = left.file_type() == gio::FileType::Directory;
        let right_dir = right.file_type() == gio::FileType::Directory;
        if left_dir != right_dir {
            return if left_dir {
                gtk4::Ordering::Smaller
            } else {
                gtk4::Ordering::Larger
            };
        }
        match left
            .display_name()
            .to_lowercase()
            .cmp(&right.display_name().to_lowercase())
        {
            std::cmp::Ordering::Less => gtk4::Ordering::Smaller,
            std::cmp::Ordering::Equal => gtk4::Ordering::Equal,
            std::cmp::Ordering::Greater => gtk4::Ordering::Larger,
        }
    });
    gtk4::SortListModel::new(Some(directory), Some(sorter))
}

pub(super) fn tree_factory(
    actions: TreeActions,
    last_click: Rc<RefCell<Option<RowClick>>>,
) -> gtk4::SignalListItemFactory {
    let factory = gtk4::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let expander = gtk4::TreeExpander::new();
        let label = gtk4::Label::new(None);
        label.set_halign(gtk4::Align::Start);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        expander.set_child(Some(&label));
        item.set_child(Some(&expander));

        let open = actions.open.clone();
        let browse = actions.browse.clone();
        let last_click = last_click.clone();
        let primary = gtk4::GestureClick::new();
        primary.set_button(gtk4::gdk::BUTTON_PRIMARY);
        primary.set_propagation_phase(gtk4::PropagationPhase::Capture);
        primary.connect_pressed(move |gesture, n_press, _, _| {
            let Some(widget) = gesture.widget() else {
                return;
            };
            let Some(path) = path_of_row_widget(&widget) else {
                return;
            };
            let now = std::time::Instant::now();
            let repeated = last_click.borrow().as_ref().is_some_and(|click| {
                click.path == path && now.duration_since(click.at) <= Duration::from_millis(450)
            });
            *last_click.borrow_mut() = Some(RowClick {
                path: path.clone(),
                at: now,
            });
            if n_press < 2 && !repeated {
                return;
            }
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            if path.is_file() {
                open(path);
            } else {
                browse(path);
            }
        });
        expander.add_controller(primary);

        let menu_actions = actions.clone();
        let secondary = gtk4::GestureClick::new();
        secondary.set_button(gtk4::gdk::BUTTON_SECONDARY);
        secondary.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk4::EventSequenceState::Claimed);
            let Some(widget) = gesture.widget() else {
                return;
            };
            let Some(path) = path_of_row_widget(&widget) else {
                return;
            };
            show_tree_menu(&widget, x, y, path, menu_actions.clone());
        });
        expander.add_controller(secondary);
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let Some(expander) = item
            .child()
            .and_then(|child| child.downcast::<gtk4::TreeExpander>().ok())
        else {
            return;
        };
        let Some(row) = item
            .item()
            .and_then(|item| item.downcast::<gtk4::TreeListRow>().ok())
        else {
            return;
        };
        expander.set_list_row(Some(&row));
        if let Some(label) = expander
            .child()
            .and_then(|child| child.downcast::<gtk4::Label>().ok())
        {
            let name = row
                .item()
                .and_then(|item| item.downcast::<gio::FileInfo>().ok())
                .map(|info| info.display_name().to_string())
                .unwrap_or_default();
            label.set_text(&name);
        }
    });
    factory.connect_unbind(|_, item| {
        if let Some(expander) = item
            .downcast_ref::<gtk4::ListItem>()
            .and_then(|item| item.child())
            .and_then(|child| child.downcast::<gtk4::TreeExpander>().ok())
        {
            expander.set_list_row(None::<&gtk4::TreeListRow>);
        }
    });
    factory
}

pub(super) fn caption(title: &str, child: &impl IsA<gtk4::Widget>) -> gtk4::Box {
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let label = gtk4::Label::new(Some(title));
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(8);
    label.set_margin_top(4);
    label.add_css_class("heading");
    box_.append(&label);
    box_.append(child);
    box_.set_hexpand(true);
    box_.set_vexpand(true);
    box_
}

pub(super) fn tree_row_of_widget(widget: &gtk4::Widget) -> Option<gtk4::TreeListRow> {
    widget
        .clone()
        .downcast::<gtk4::TreeExpander>()
        .ok()
        .or_else(|| widget.parent().and_then(|parent| parent.downcast().ok()))?
        .list_row()
}

pub(super) fn path_of_row_widget(widget: &gtk4::Widget) -> Option<PathBuf> {
    let row = tree_row_of_widget(widget)?;
    let info = row.item()?.downcast::<gio::FileInfo>().ok()?;
    let file = info
        .attribute_object("standard::file")?
        .downcast::<gio::File>()
        .ok()?;
    file.path()
}

pub(super) fn show_tree_menu(
    anchor: &gtk4::Widget,
    x: f64,
    y: f64,
    path: PathBuf,
    actions: TreeActions,
) {
    let popover = gtk4::Popover::new();
    popover.add_css_class("context-menu");
    popover.set_parent(anchor);
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk4::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    let menu = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    if path.is_file() {
        let open = menu_row("Открыть");
        let popover_close = popover.clone();
        let target = path.clone();
        let on_open = actions.open.clone();
        open.connect_clicked(move |_| {
            on_open(target.clone());
            popover_close.popdown();
        });
        menu.append(&open);
        let title = if (actions.is_pinned)(&path) {
            "Открепить файл"
        } else {
            "Закрепить файл"
        };
        let pin_file = menu_row(title);
        let popover_close = popover.clone();
        let target = path.clone();
        let on_pin_file = actions.pin_file.clone();
        pin_file.connect_clicked(move |_| {
            on_pin_file(target.clone());
            popover_close.popdown();
        });
        menu.append(&pin_file);
    }
    let pin_path = if path.is_dir() {
        path.clone()
    } else {
        path.parent().unwrap_or(path.as_path()).to_path_buf()
    };
    let pin_title = if path.is_dir() {
        "Запинить пространство"
    } else {
        "Запинить каталог файла"
    };
    let pin = menu_row(pin_title);
    let popover_close = popover.clone();
    let on_pin = actions.pin.clone();
    pin.connect_clicked(move |_| {
        on_pin(pin_path.clone());
        popover_close.popdown();
    });
    menu.append(&pin);
    let folder = if path.is_dir() {
        path.clone()
    } else {
        path.parent().unwrap_or(path.as_path()).to_path_buf()
    };
    let mkfile = menu_row("Создать файл");
    let popover_close = popover.clone();
    let on_mkfile = actions.mkfile.clone();
    let anchor_row = anchor.clone();
    let expand_here = path.is_dir();
    let file_parent = folder.clone();
    mkfile.connect_clicked(move |_| {
        if expand_here {
            if let Some(row) = tree_row_of_widget(&anchor_row) {
                row.set_expanded(true);
            }
        }
        on_mkfile(file_parent.clone());
        popover_close.popdown();
    });
    menu.append(&mkfile);
    let mkdir = menu_row("Создать каталог");
    let popover_close = popover.clone();
    let on_mkdir = actions.mkdir.clone();
    let anchor_row = anchor.clone();
    mkdir.connect_clicked(move |_| {
        if expand_here {
            if let Some(row) = tree_row_of_widget(&anchor_row) {
                row.set_expanded(true);
            }
        }
        on_mkdir(folder.clone());
        popover_close.popdown();
    });
    menu.append(&mkdir);
    let contents = menu_row("Показать содержимое");
    let popover_close = popover.clone();
    let target = path.clone();
    let browse = actions.browse.clone();
    contents.connect_clicked(move |_| {
        browse(target.clone());
        popover_close.popdown();
    });
    menu.append(&contents);
    let copy = menu_row("Копировать путь");
    let popover_close = popover.clone();
    let target = path.clone();
    let on_notice = actions.notice.clone();
    copy.connect_clicked(move |_| {
        if let Some(display) = gtk4::gdk::Display::default() {
            display.clipboard().set_text(&target.display().to_string());
        }
        on_notice("Путь скопирован".to_string());
        popover_close.popdown();
    });
    menu.append(&copy);
    let disk = menu_row("Показать в проводнике");
    let popover_close = popover.clone();
    let target = path.clone();
    let reveal_disk = actions.reveal_disk.clone();
    disk.connect_clicked(move |_| {
        reveal_disk(target.clone());
        popover_close.popdown();
    });
    menu.append(&disk);
    let workspace = menu_row("Показать в пространстве");
    let popover_close = popover.clone();
    let target = path.clone();
    let reveal_workspace = actions.reveal_workspace.clone();
    workspace.connect_clicked(move |_| {
        reveal_workspace(target.clone());
        popover_close.popdown();
    });
    menu.append(&workspace);
    popover.set_child(Some(&menu));
    popover.connect_closed(|popover| {
        popover.unparent();
    });
    popover.popup();
}

pub(super) const REVEAL_TRIES: u8 = 40;

pub(super) enum Scan {
    Pending,
    Missing,
    Found(gtk4::TreeListRow),
}

pub(super) struct Reveal {
    list: gtk4::ListView,
    names: Vec<OsString>,
    index: usize,
    parent: Option<gtk4::TreeListRow>,
    attempts: u8,
    target: PathBuf,
    notice: Rc<dyn Fn(String)>,
}

pub(super) fn reveal_in_tree(
    list: gtk4::ListView,
    root: PathBuf,
    target: PathBuf,
    notice: Rc<dyn Fn(String)>,
) {
    if !target.starts_with(&root) {
        notice("Путь вне этого дерева".to_string());
        return;
    }
    let relative = target.strip_prefix(&root).unwrap_or(target.as_path());
    let names: Vec<OsString> = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_os_string()),
            _ => None,
        })
        .collect();
    if names.is_empty() {
        notice("Это корень дерева".to_string());
        return;
    }
    Reveal {
        list,
        names,
        index: 0,
        parent: None,
        attempts: 0,
        target,
        notice,
    }
    .schedule();
}

impl Reveal {
    fn schedule(self) {
        glib::timeout_add_local_once(Duration::from_millis(50), move || self.step());
    }

    fn step(mut self) {
        let wanted = self.names[self.index].clone();
        match self.scan(wanted.as_os_str()) {
            Scan::Pending => self.retry(),
            Scan::Missing => {
                (self.notice)(format!("В каталоге нет «{}»", wanted.to_string_lossy()));
            }
            Scan::Found(row) => {
                let last = self.index + 1 == self.names.len();
                if last {
                    if self.target.is_dir() {
                        row.set_expanded(true);
                    }
                    scroll_when_ready(self.list, self.target, self.notice, 0);
                    return;
                }
                row.set_expanded(true);
                self.parent = Some(row);
                self.index += 1;
                self.attempts = 0;
                self.schedule();
            }
        }
    }

    fn retry(mut self) {
        self.attempts += 1;
        if self.attempts > REVEAL_TRIES {
            (self.notice)("Не удалось раскрыть каталог".to_string());
            return;
        }
        self.schedule();
    }

    fn scan(&self, wanted: &OsStr) -> Scan {
        if let Some(parent) = &self.parent {
            if !parent.is_expanded() {
                parent.set_expanded(true);
                return Scan::Pending;
            }
            let Some(children) = parent.children() else {
                return Scan::Missing;
            };
            return scan_children(parent, &children, wanted);
        }
        let Some(model) = self.list.model() else {
            return Scan::Pending;
        };
        if model.n_items() == 0 {
            return if root_is_loading(&self.list) {
                Scan::Pending
            } else if self.attempts < 3 {
                Scan::Pending
            } else {
                Scan::Missing
            };
        }
        for position in 0..model.n_items() {
            let Some(item) = model.item(position) else {
                continue;
            };
            let Ok(row) = item.downcast::<gtk4::TreeListRow>() else {
                continue;
            };
            if row.depth() != 0 {
                continue;
            }
            if row_file_name(&row).as_deref() == Some(wanted) {
                return Scan::Found(row);
            }
        }
        if root_is_loading(&self.list) || self.attempts < 3 {
            Scan::Pending
        } else {
            Scan::Missing
        }
    }
}

pub(super) fn scan_children(
    parent: &gtk4::TreeListRow,
    children: &gio::ListModel,
    wanted: &OsStr,
) -> Scan {
    if children.n_items() == 0 {
        return if directory_loading(children) {
            Scan::Pending
        } else {
            Scan::Missing
        };
    }
    let mut saw_row = false;
    for position in 0..children.n_items() {
        let Some(row) = parent.child_row(position) else {
            continue;
        };
        saw_row = true;
        if row_file_name(&row).as_deref() == Some(wanted) {
            return Scan::Found(row);
        }
    }
    if !saw_row || directory_loading(children) {
        Scan::Pending
    } else {
        Scan::Missing
    }
}

pub(super) fn scroll_when_ready(
    list: gtk4::ListView,
    target: PathBuf,
    notice: Rc<dyn Fn(String)>,
    attempts: u8,
) {
    if scroll_to_path(&list, &target) {
        notice("Каталог показан в дереве".to_string());
        return;
    }
    if attempts > REVEAL_TRIES {
        notice("Каталог раскрыт, строка ещё не видна".to_string());
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(50), move || {
        scroll_when_ready(list, target, notice, attempts + 1);
    });
}

pub(super) fn scroll_to_path(list: &gtk4::ListView, target: &Path) -> bool {
    let Some(model) = list.model() else {
        return false;
    };
    for position in 0..model.n_items() {
        let Some(item) = model.item(position) else {
            continue;
        };
        let Ok(row) = item.downcast::<gtk4::TreeListRow>() else {
            continue;
        };
        if row_path(&row).as_deref() != Some(target) {
            continue;
        }
        if let Some(selection) = model.downcast_ref::<gtk4::SingleSelection>() {
            selection.set_selected(position);
        }
        list.scroll_to(
            position,
            gtk4::ListScrollFlags::FOCUS | gtk4::ListScrollFlags::SELECT,
            None,
        );
        return true;
    }
    false
}

pub(super) fn root_is_loading(list: &gtk4::ListView) -> bool {
    let Some(selection) = list
        .model()
        .and_then(|model| model.downcast::<gtk4::SingleSelection>().ok())
    else {
        return true;
    };
    let Ok(tree) = selection
        .model()
        .ok_or(())
        .and_then(|model| model.downcast::<gtk4::TreeListModel>().map_err(|_| ()))
    else {
        return true;
    };
    directory_loading(&tree.model())
}

pub(super) fn directory_loading(model: &gio::ListModel) -> bool {
    if let Some(directory) = model.downcast_ref::<gtk4::DirectoryList>() {
        return directory.is_loading();
    }
    let Some(sorted) = model.downcast_ref::<gtk4::SortListModel>() else {
        return false;
    };
    sorted
        .model()
        .and_then(|inner| inner.downcast::<gtk4::DirectoryList>().ok())
        .is_some_and(|directory| directory.is_loading())
}

pub(super) fn row_file_name(row: &gtk4::TreeListRow) -> Option<OsString> {
    row_path(row)?.file_name().map(|name| name.to_os_string())
}

pub(super) fn row_path(row: &gtk4::TreeListRow) -> Option<PathBuf> {
    let info = row.item()?.downcast::<gio::FileInfo>().ok()?;
    let file = info
        .attribute_object("standard::file")?
        .downcast::<gio::File>()
        .ok()?;
    file.path()
}

pub(super) fn menu_row(title: &str) -> gtk4::Button {
    let button = gtk4::Button::with_label(title);
    button.set_has_frame(false);
    button.set_halign(gtk4::Align::Fill);
    button.add_css_class("menu-row");
    if let Some(label) = button
        .first_child()
        .and_then(|child| child.downcast::<gtk4::Label>().ok())
    {
        label.set_xalign(0.0);
        label.set_halign(gtk4::Align::Start);
        label.set_hexpand(true);
    }
    button
}

pub(super) fn watch_file(path: &Path, ctx: Ctx) -> Option<gio::FileMonitor> {
    let file = gio::File::for_path(path);
    let monitor = file
        .monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
        .ok()?;
    let watch = path.to_path_buf();
    monitor.connect_changed(move |_, _, _, event| {
        if matches!(
            event,
            gio::FileMonitorEvent::ChangesDoneHint
                | gio::FileMonitorEvent::Changed
                | gio::FileMonitorEvent::Created
                | gio::FileMonitorEvent::Renamed
        ) {
            ctx.on_disk_changed(&watch);
        }
    });
    Some(monitor)
}

pub(super) fn notebook_for(
    side: seams::PanelSide,
    pages: &[(&str, gtk4::Widget)],
) -> gtk4::Notebook {
    let notebook = gtk4::Notebook::new();
    notebook.set_vexpand(true);
    for spec in seams::panels().iter().filter(|panel| panel.side == side) {
        let Some((_, child)) = pages.iter().find(|(id, _)| *id == spec.id) else {
            continue;
        };
        notebook.append_page(child, Some(&gtk4::Label::new(Some(spec.title))));
    }
    notebook
}

pub(super) fn shell_link(link: markdown::LocalLink) -> String {
    match link {
        markdown::LocalLink::Open { path, line } => {
            let encoded = markdown::encode_path(&path.to_string_lossy());
            match line {
                Some(line) => format!("twopen:{encoded}?line={line}"),
                None => format!("twopen:{encoded}"),
            }
        }
        markdown::LocalLink::Missing { label } => {
            format!("twmissing:{}", markdown::encode_path(&label))
        }
        markdown::LocalLink::Pick { name } => format!("twpick:{}", markdown::encode_path(&name)),
        markdown::LocalLink::Tag { name } => format!("twtag:{}", markdown::encode_path(&name)),
    }
}

pub(super) fn tip_tab(notebook: &gtk4::Notebook, index: u32, text: &str) {
    let Some(page) = notebook.nth_page(Some(index)) else {
        return;
    };
    if let Some(label) = notebook.tab_label(&page) {
        label.set_tooltip_text(Some(text));
    }
}

pub(super) fn replace_notebook_page(
    notebook: &gtk4::Notebook,
    index: u32,
    child: &impl IsA<gtk4::Widget>,
) {
    let title = notebook
        .tab_label(
            &notebook
                .nth_page(Some(index))
                .unwrap_or_else(|| child.as_ref().clone()),
        )
        .and_then(|label| label.downcast::<gtk4::Label>().ok())
        .map(|label| label.text().to_string())
        .unwrap_or_else(|| "Диск".to_string());
    if let Some(page) = notebook.nth_page(Some(index)) {
        notebook.remove_page(Some(index));
        let _ = page;
    }
    notebook.insert_page(child, Some(&gtk4::Label::new(Some(&title))), Some(index));
}

pub(super) fn side_caption(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.add_css_class("side-caption");
    label.set_halign(gtk4::Align::Start);
    label
}

pub(super) fn dim_row(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(8);
    label.set_margin_top(4);
    label.set_margin_bottom(4);
    label
}

pub(super) fn scrolled(child: &impl IsA<gtk4::Widget>) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .child(child)
        .hexpand(true)
        .vexpand(true)
        .build()
}

pub(super) fn clear_box(box_: &gtk4::Box) {
    while let Some(child) = box_.first_child() {
        box_.remove(&child);
    }
}

pub(super) fn clear_list(list: &gtk4::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

pub(super) fn paint_graph(
    area: &gtk4::DrawingArea,
    cr: &gtk4::cairo::Context,
    layout: &graph::GraphLayout,
) {
    let color = area.color();
    cr.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        0.45,
    );
    cr.set_line_width(1.0);
    for edge in &layout.edges {
        let Some(from) = layout.nodes.get(edge.from) else {
            continue;
        };
        let Some(to) = layout.nodes.get(edge.to) else {
            continue;
        };
        cr.move_to(from.x, from.y);
        cr.line_to(to.x, to.y);
        cr.stroke().ok();
    }
    let _ = cr.select_font_face(
        "Sans",
        gtk4::cairo::FontSlant::Normal,
        gtk4::cairo::FontWeight::Normal,
    );
    cr.set_font_size(11.0);
    if layout.nodes.is_empty() {
        cr.move_to(12.0, 24.0);
        cr.show_text("Нет связей в активном пространстве").ok();
        return;
    }
    for node in &layout.nodes {
        if node.current {
            cr.set_source_rgba(0.2, 0.45, 0.85, 1.0);
        } else {
            cr.set_source_rgba(
                f64::from(color.red()),
                f64::from(color.green()),
                f64::from(color.blue()),
                0.9,
            );
        }
        cr.arc(
            node.x,
            node.y,
            if node.current { 8.0 } else { 5.0 },
            0.0,
            std::f64::consts::TAU,
        );
        cr.fill().ok();
        cr.move_to(node.x + 10.0, node.y + 4.0);
        cr.show_text(&node.label).ok();
    }
}

pub(super) fn read_text(path: &Path) -> Result<String, String> {
    let meta = fs::metadata(path).map_err(|error| error.to_string())?;
    if !meta.is_file() {
        return Err("Это не файл".to_string());
    }
    if meta.len() > MAX_OPEN_BYTES {
        return Err("Файл больше 8 МБ".to_string());
    }
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    if bytes.contains(&0) {
        return Err("Это не текстовый файл".to_string());
    }
    String::from_utf8(bytes).map_err(|_| "Файл не в UTF-8".to_string())
}

/// Запись в тот же inode. Подмена файла через rename даёт новую дату создания.
pub(super) fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let bytes = text.as_bytes();
    let mut file = if path.exists() {
        OpenOptions::new().write(true).open(path)?
    } else {
        OpenOptions::new().write(true).create_new(true).open(path)?
    };
    file.write_all(bytes)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()
}

pub(super) fn buffer_text_of(buffer: &sourceview5::Buffer) -> String {
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.text(&start, &end, false).to_string()
}

pub(super) fn about_rows(
    path: &Path,
    text: Option<&str>,
    dirty: Option<bool>,
) -> Vec<(&'static str, String)> {
    vec![
        ("Тип", type_label(path)),
        ("Размер", size_of(path)),
        ("Кодировка", encoding_label(path, text)),
        ("Создан", time_of(path, TimeField::Created)),
        ("Отредактирован", time_of(path, TimeField::Modified)),
        ("Строк", lines_label(text)),
        ("Правки", edits_label(dirty)),
    ]
}

enum TimeField {
    Created,
    Modified,
}

fn type_label(path: &Path) -> String {
    let base = match kind::shown(path) {
        kind::Shown::Html => "HTML".to_string(),
        kind::Shown::Picture | kind::Shown::WebImage => {
            format!("Изображение {}", ext_upper(path))
        }
        kind::Shown::Pdf => "PDF".to_string(),
        kind::Shown::Audio => format!("Аудио {}", ext_upper(path)),
        kind::Shown::Video => format!("Видео {}", ext_upper(path)),
        kind::Shown::Svg => "SVG".to_string(),
        kind::Shown::Text => text_type(path),
    };
    if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        format!("Ссылка, {base}")
    } else {
        base
    }
}

fn text_type(path: &Path) -> String {
    match ext_lower(path).as_deref() {
        Some("md" | "markdown") => "Markdown".to_string(),
        Some("txt") | None => "Текст".to_string(),
        Some("json") => "JSON".to_string(),
        Some("rs") => "Rust".to_string(),
        Some("py") => "Python".to_string(),
        Some("toml") => "TOML".to_string(),
        Some("yml" | "yaml") => "YAML".to_string(),
        Some("xml") => "XML".to_string(),
        Some("csv") => "CSV".to_string(),
        Some("css") => "CSS".to_string(),
        Some("js" | "mjs") => "JavaScript".to_string(),
        Some("ts") => "TypeScript".to_string(),
        Some("sh") => "Скрипт".to_string(),
        Some(ext) => ext.to_ascii_uppercase(),
    }
}

fn ext_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
}

fn ext_upper(path: &Path) -> String {
    ext_lower(path)
        .map(|ext| ext.to_ascii_uppercase())
        .unwrap_or_default()
}

fn size_of(path: &Path) -> String {
    fs::metadata(path)
        .map(|meta| format_size(meta.len()))
        .unwrap_or_else(|_| "—".to_string())
}

fn encoding_label(path: &Path, text: Option<&str>) -> String {
    let Some(text) = text else {
        return "—".to_string();
    };
    if kind::is_view_only(path) {
        return "—".to_string();
    }
    if text.starts_with('\u{feff}') {
        "UTF-8 с BOM".to_string()
    } else {
        "UTF-8".to_string()
    }
}

fn time_of(path: &Path, field: TimeField) -> String {
    let Ok(meta) = fs::metadata(path) else {
        return "—".to_string();
    };
    let stamp = match field {
        TimeField::Created => meta.created().ok(),
        TimeField::Modified => meta.modified().ok(),
    };
    stamp.map(format_system_time).unwrap_or_else(|| "—".to_string())
}

fn format_system_time(time: std::time::SystemTime) -> String {
    let Ok(since) = time.duration_since(std::time::UNIX_EPOCH) else {
        return "—".to_string();
    };
    let Ok(local) = glib::DateTime::from_unix_local(since.as_secs() as i64) else {
        return "—".to_string();
    };
    local
        .format("%d.%m.%Y %H:%M")
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "—".to_string())
}

fn lines_label(text: Option<&str>) -> String {
    match text {
        Some(text) => text.lines().count().to_string(),
        None => "—".to_string(),
    }
}

fn edits_label(dirty: Option<bool>) -> String {
    match dirty {
        Some(true) => "не сохранены".to_string(),
        Some(false) => "нет".to_string(),
        None => "—".to_string(),
    }
}

pub(super) fn directory_name(raw: &str) -> Result<&str, &'static str> {
    leaf_name(raw, "Введите имя каталога")
}

pub(super) fn file_name(raw: &str) -> Result<&str, &'static str> {
    leaf_name(raw, "Введите имя файла")
}

fn leaf_name<'a>(raw: &'a str, empty: &'static str) -> Result<&'a str, &'static str> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(empty);
    }
    if name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err("В имени не должно быть пути");
    }
    Ok(name)
}

pub(super) fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown")
    )
}

pub(super) const LAUNCHER_ICON: &[u8] = include_bytes!("../../packaging/verst.png");

pub(super) fn publish_window_icon(window: &gtk4::ApplicationWindow) {
    let Some(icon_path) = install_launcher_icon() else {
        window.set_icon_name(Some("verst"));
        return;
    };
    let theme = gtk4::IconTheme::for_display(&gtk4::prelude::WidgetExt::display(window));
    if theme.has_icon("verst") {
        window.set_icon_name(Some("verst"));
    } else {
        window.set_icon_name(Some(&icon_path.to_string_lossy()));
    }
}

pub(super) fn install_launcher_icon() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    if home.as_os_str().is_empty() {
        return None;
    }
    let icon_dir = home.join(".local/share/icons/hicolor/256x256/apps");
    let theme_icon = icon_dir.join("verst.png");
    // Ubuntu Dock keeps the first pixmap for a path, and paints transparent
    // pixels on white. A new file is what actually reaches the tray.
    let icon_path = icon_dir.join("verstgem.png");
    let desktop_dir = home.join(".local/share/applications");
    let desktop_path = desktop_dir.join("local.textworkspace.desktop");
    fs::create_dir_all(&icon_dir).ok()?;
    fs::create_dir_all(&desktop_dir).ok()?;
    if fs::read(&theme_icon).ok().as_deref() != Some(LAUNCHER_ICON) {
        fs::write(&theme_icon, LAUNCHER_ICON).ok()?;
    }
    if fs::read(&icon_path).ok().as_deref() != Some(LAUNCHER_ICON) {
        fs::write(&icon_path, LAUNCHER_ICON).ok()?;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("text_workspace"));
    let desktop = format!(
        "[Desktop Entry]\n\
Type=Application\n\
Name=Verst\n\
GenericName=Text Workspace\n\
Comment=Исходник и страница одного файла\n\
Exec={exec} %U\n\
Icon={icon}\n\
Terminal=false\n\
Categories=Utility;TextEditor;\n\
MimeType=text/plain;text/markdown;text/x-markdown;\n\
StartupWMClass=local.textworkspace\n\
Keywords=markdown;notes;editor;\n",
        exec = exe.display(),
        icon = icon_path.display(),
    );
    if fs::read_to_string(&desktop_path).ok().as_deref() != Some(desktop.as_str()) {
        fs::write(&desktop_path, &desktop).ok()?;
        let _ = std::process::Command::new("update-desktop-database")
            .arg(&desktop_dir)
            .status();
    }
    Some(icon_path)
}

pub(super) fn scroll_top_for_line(line_top: f64, page: f64, upper: f64) -> f64 {
    let target = line_top - page / 3.0;
    target.clamp(0.0, (upper - page).max(0.0))
}

/// Строка на границе трети экрана. Погрешность в пару пикселей не должна
/// перескакивать на предыдущую строку, иначе каждое переключение уезжает вверх.
pub(super) fn anchor_line_at(sample_y: i32, line: i32, next_top: Option<i32>) -> i32 {
    let Some(next_top) = next_top else {
        return line;
    };
    let gap = next_top - sample_y;
    if (0..=3).contains(&gap) {
        line + 1
    } else {
        line
    }
}

pub(super) fn scrolled_of(widget: &impl IsA<gtk4::Widget>) -> Option<gtk4::ScrolledWindow> {
    let mut current = widget.parent();
    while let Some(widget) = current {
        if let Ok(scroll) = widget.clone().downcast::<gtk4::ScrolledWindow>() {
            return Some(scroll);
        }
        current = widget.parent();
    }
    None
}

pub(super) fn file_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("файл")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_rows_describe_a_markdown_note() {
        let dir = std::env::temp_dir().join(format!("verst_about_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        let body = "\u{feff}# Заголовок\nтекст\n";
        fs::write(&path, body).unwrap();
        let rows: std::collections::HashMap<_, _> =
            about_rows(&path, Some(body), Some(true)).into_iter().collect();
        assert_eq!(rows["Тип"], "Markdown");
        assert_eq!(rows["Кодировка"], "UTF-8 с BOM");
        assert_eq!(rows["Строк"], "2");
        assert_eq!(rows["Правки"], "не сохранены");
        assert!(rows["Размер"].ends_with("Б"));
        assert_ne!(rows["Отредактирован"], "—");
        assert_eq!(encoding_label(Path::new("photo.jpg"), None), "—");
        assert_eq!(type_label(Path::new("page.html")), "HTML");
        assert_eq!(type_label(Path::new("icon.svg")), "SVG");
        assert_eq!(directory_name("  заметки  ").unwrap(), "заметки");
        assert!(directory_name(" ").is_err());
        assert!(directory_name("../секрет").is_err());
        assert!(directory_name("a/b").is_err());
        assert_eq!(file_name("  заметка.md  ").unwrap(), "заметка.md");
        assert_eq!(file_name("").unwrap_err(), "Введите имя файла");
        assert!(file_name(".gitignore").is_ok());
        assert!(file_name("..").is_err());
        assert!(file_name("a\\b").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrite_keeps_creation_time() {
        let dir = std::env::temp_dir().join(format!("verst_btime_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("note.md");
        fs::write(&path, "было коротко").unwrap();
        let created = fs::metadata(&path).unwrap().created().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        write_atomic(&path, "стало").unwrap();
        let after_short = fs::metadata(&path).unwrap();
        assert_eq!(after_short.created().unwrap(), created);
        assert!(after_short.modified().unwrap() >= created);
        assert_eq!(fs::read_to_string(&path).unwrap(), "стало");
        write_atomic(&path, "стало заметно длиннее, чем исходный текст").unwrap();
        let after_long = fs::metadata(&path).unwrap();
        assert_eq!(after_long.created().unwrap(), created);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "стало заметно длиннее, чем исходный текст"
        );
        let fresh = dir.join("new.md");
        write_atomic(&fresh, "новый").unwrap();
        assert_eq!(fs::read_to_string(&fresh).unwrap(), "новый");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn anchor_line_stays_a_third_down() {
        assert_eq!(scroll_top_for_line(300.0, 300.0, 2000.0), 200.0);
        assert_eq!(scroll_top_for_line(10.0, 300.0, 2000.0), 0.0);
        assert_eq!(scroll_top_for_line(1900.0, 300.0, 2000.0), 1700.0);
        assert_eq!(anchor_line_at(499, 9, Some(500)), 10);
        assert_eq!(anchor_line_at(500, 10, Some(522)), 10);
        assert_eq!(anchor_line_at(510, 10, Some(522)), 10);
    }
}
