use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gtk4::gio;
use gtk4::prelude::*;

use text_workspace::markdown;
use text_workspace::notes;

use super::support::{file_icon, is_markdown, write_atomic};
use super::Ctx;

impl Ctx {
    pub(super) fn quick_note(&self) {
        let dir = notes::notes_dir();
        if let Err(error) = fs::create_dir_all(&dir) {
            self.set_status(&format!("Не удалось открыть каталог заметок: {error}"));
            return;
        }
        let ctx = self.clone();
        self.prompt_entry(
            "Быстрая заметка",
            "Название",
            dir,
            move |_dir, raw| ctx.create_quick_note(raw),
        );
    }

    fn create_quick_note(&self, raw: &str) -> bool {
        let dir = notes::notes_dir();
        if let Err(error) = fs::create_dir_all(&dir) {
            self.set_status(&format!("Не удалось открыть каталог заметок: {error}"));
            return false;
        }
        let existing = note_file_names(&dir);
        let name = notes::unique_note_name(raw, &existing);
        let path = dir.join(&name);
        if let Err(error) = write_atomic(&path, &notes::note_body(raw)) {
            self.set_status(&format!("Не удалось создать заметку: {error}"));
            return false;
        }
        self.show_panel("notes");
        self.go(path, None);
        self.refresh_notes();
        self.set_status("Заметка создана. Теги пишутся в тексте: #тема");
        true
    }

    pub(super) fn refresh_notes(&self) {
        let dir = notes::notes_dir();
        let filter = self.state.borrow().session.note_tag.clone();
        let mut found = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() || !is_markdown(&path) {
                    continue;
                }
                let text = self.note_text(&path);
                let tags = markdown::extract_tags(&text);
                let stem = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("заметка")
                    .to_string();
                let meta = entry.metadata().ok();
                let modified = meta
                    .as_ref()
                    .and_then(|meta| meta.modified().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let created = meta.as_ref().and_then(|meta| meta.created().ok());
                found.push(InboxNote {
                    path,
                    label: notes::note_label(&text, &stem),
                    tags,
                    created,
                    modified,
                });
            }
        }
        found.sort_by(|left, right| right.modified.cmp(&left.modified));
        let mut filter = filter;
        if filter.as_ref().is_some_and(|tag| {
            !found
                .iter()
                .any(|note| notes::has_tag(&note.tags, Some(tag)))
        }) {
            self.state.borrow_mut().session.note_tag = None;
            self.save_session();
            filter = None;
        }
        self.fill_note_tags(&found, filter.as_deref());
        let group = notes::note_group(&self.state.borrow().session.note_group);
        self.mark_note_group(group);
        let mut visible: Vec<_> = found
            .into_iter()
            .filter(|note| notes::has_tag(&note.tags, filter.as_deref()))
            .collect();
        sort_notes(&mut visible, group);
        clear_note_host(&self.widgets.notes_host);
        if visible.is_empty() {
            self.widgets
                .notes_host
                .append(&note_empty_row(filter.is_some()));
            return;
        }
        if group == notes::NoteGroup::Flat {
            for note in &visible {
                self.widgets.notes_host.append(&self.note_row(note));
            }
            return;
        }
        if group == notes::NoteGroup::Tags {
            self.append_note_tags(&visible);
            return;
        }
        self.append_note_tree(&visible, group);
    }

    fn mark_note_group(&self, group: notes::NoteGroup) {
        for (button, mode) in [
            (&self.widgets.notes_group_flat, notes::NoteGroup::Flat),
            (
                &self.widgets.notes_group_created,
                notes::NoteGroup::Created,
            ),
            (
                &self.widgets.notes_group_modified,
                notes::NoteGroup::Modified,
            ),
            (&self.widgets.notes_group_tags, notes::NoteGroup::Tags),
        ] {
            if mode == group {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
        }
    }

    pub(super) fn set_note_group(&self, group: &str) {
        let group = notes::note_group(group).as_str().to_string();
        self.state.borrow_mut().session.note_group = group;
        self.widgets.notes_menu.popdown();
        self.refresh_notes();
        self.save_session();
    }

    fn append_note_tree(&self, notes: &[InboxNote], group: notes::NoteGroup) {
        let stamps: Vec<_> = notes.iter().map(|note| note_stamp(note, group)).collect();
        let days = notes::group_by_day(&stamps);
        let mut index = 0;
        while index < days.len() {
            let year = days[index].year;
            if year == 0 {
                let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                for note_index in &days[index].indexes {
                    if let Some(note) = notes.get(*note_index) {
                        box_.append(&self.note_row(note));
                    }
                }
                self.widgets
                    .notes_host
                    .append(&note_branch("Без даты", &box_));
                index += 1;
                continue;
            }
            let year_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            while index < days.len() && days[index].year == year {
                let month = days[index].month;
                let month_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                month_box.add_css_class("note-branch");
                while index < days.len() && days[index].year == year && days[index].month == month
                {
                    let day_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
                    day_box.add_css_class("note-branch");
                    for note_index in &days[index].indexes {
                        if let Some(note) = notes.get(*note_index) {
                            day_box.append(&self.note_row(note));
                        }
                    }
                    month_box.append(&note_branch(&days[index].day.to_string(), &day_box));
                    index += 1;
                }
                year_box.append(&note_branch(month_name(month), &month_box));
            }
            self.widgets
                .notes_host
                .append(&note_branch(&year.to_string(), &year_box));
        }
    }

    fn append_note_tags(&self, notes: &[InboxNote]) {
        let tags: Vec<Vec<String>> = notes.iter().map(|note| note.tags.clone()).collect();
        for group in notes::group_by_tag(&tags) {
            let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
            box_.add_css_class("note-branch");
            for note_index in &group.indexes {
                if let Some(note) = notes.get(*note_index) {
                    box_.append(&self.note_row(note));
                }
            }
            let title = group
                .tag
                .as_deref()
                .map(|tag| format!("#{tag}"))
                .unwrap_or_else(|| "Без тега".to_string());
            self.widgets.notes_host.append(&note_branch(&title, &box_));
        }
    }

    fn fill_note_tags(&self, notes: &[InboxNote], filter: Option<&str>) {
        while let Some(child) = self.widgets.notes_tags.first_child() {
            self.widgets.notes_tags.remove(&child);
        }
        let mut tags = Vec::new();
        for note in notes {
            for tag in &note.tags {
                if !tags.contains(tag) {
                    tags.push(tag.clone());
                }
            }
        }
        tags.sort();
        self.widgets.notes_tags_fold.set_visible(!tags.is_empty());
        self.widgets.notes_tags_all.set_visible(tags.len() > 3);
        self.widgets.notes_tags.append(&self.note_tag_chip(None, filter));
        for tag in tags {
            self.widgets
                .notes_tags
                .append(&self.note_tag_chip(Some(tag), filter));
        }
    }

    fn note_tag_chip(&self, tag: Option<String>, filter: Option<&str>) -> gtk4::Button {
        let title = tag.as_deref().map(|tag| format!("#{tag}")).unwrap_or_else(|| "Все".to_string());
        let button = gtk4::Button::with_label(&title);
        button.add_css_class("flat");
        button.add_css_class("tag-chip");
        let selected = match (&tag, filter) {
            (None, None) => true,
            (Some(tag), Some(filter)) => tag == filter,
            _ => false,
        };
        if selected {
            button.add_css_class("tag-chip-on");
        }
        let ctx = self.clone();
        button.connect_clicked(move |_| {
            let next = if selected { None } else { tag.clone() };
            ctx.set_note_tag(next);
        });
        button
    }

    fn set_note_tag(&self, tag: Option<String>) {
        let tag = tag.and_then(|tag| markdown::canonical_tag(&tag));
        if self.state.borrow().session.note_tag == tag {
            self.refresh_notes();
            return;
        }
        self.state.borrow_mut().session.note_tag = tag;
        self.refresh_notes();
        self.save_session();
    }

    fn note_row(&self, note: &InboxNote) -> gtk4::Box {
        let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        row.add_css_class("note-row");
        row.add_css_class("file-row");
        row.append(&file_icon(&note.path));
        let open = gtk4::Button::with_label(&note.label);
        open.add_css_class("flat");
        open.add_css_class("row-title");
        open.set_hexpand(true);
        open.set_halign(gtk4::Align::Fill);
        if let Some(label) = open.first_child().and_downcast::<gtk4::Label>() {
            label.set_halign(gtk4::Align::Start);
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        }
        if !note.tags.is_empty() {
            let shown = note
                .tags
                .iter()
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" ");
            open.set_tooltip_text(Some(&shown));
        }
        let path = note.path.clone();
        let ctx = self.clone();
        open.connect_clicked(move |_| {
            ctx.go(path.clone(), None);
        });
        let move_note = gtk4::Button::with_label("В пространство");
        move_note.add_css_class("flat");
        move_note.add_css_class("note-move");
        move_note.set_tooltip_text(Some("Выбрать пространство, каталог и имя файла"));
        let path = note.path.clone();
        let ctx = self.clone();
        move_note.connect_clicked(move |_| ctx.move_note_to_workspace(&path));
        let delete_note = gtk4::Button::from_icon_name("user-trash-symbolic");
        delete_note.add_css_class("flat");
        delete_note.add_css_class("row-action");
        delete_note.set_tooltip_text(Some("Удалить заметку"));
        let path = note.path.clone();
        let label = note.label.clone();
        let ctx = self.clone();
        delete_note.connect_clicked(move |_| ctx.confirm_delete_note(path.clone(), label.clone()));
        row.append(&open);
        row.append(&move_note);
        row.append(&delete_note);
        row
    }

    fn confirm_delete_note(&self, path: PathBuf, label: String) {
        if !notes::is_inbox(&path) {
            self.set_status("Это не быстрая заметка");
            return;
        }
        let dialog = gtk4::Window::builder()
            .transient_for(&self.widgets.window)
            .modal(true)
            .title("Удалить заметку")
            .default_width(420)
            .build();
        dialog.add_css_class("prompt-dialog");
        let text = gtk4::Label::new(Some(&format!(
            "Удалить «{label}»? Файл исчезнет с диска."
        )));
        text.set_wrap(true);
        text.set_halign(gtk4::Align::Start);
        text.set_margin_bottom(12);
        let cancel = gtk4::Button::with_label("Отмена");
        cancel.add_css_class("flat");
        let remove = gtk4::Button::with_label("Удалить");
        remove.add_css_class("destructive-action");
        let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
        actions.set_halign(gtk4::Align::End);
        actions.append(&cancel);
        actions.append(&remove);
        let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        box_.set_margin_top(16);
        box_.set_margin_bottom(16);
        box_.set_margin_start(16);
        box_.set_margin_end(16);
        box_.append(&text);
        box_.append(&actions);
        dialog.set_child(Some(&box_));
        let close = dialog.clone();
        cancel.connect_clicked(move |_| close.close());
        let close = dialog.clone();
        let ctx = self.clone();
        remove.connect_clicked(move |_| {
            ctx.delete_note(&path);
            close.close();
        });
        dialog.present();
    }

    fn delete_note(&self, path: &Path) {
        if !notes::is_inbox(path) {
            self.set_status("Это не быстрая заметка");
            return;
        }
        let opened = self
            .state
            .borrow()
            .session
            .open_files
            .iter()
            .any(|open| open == path)
            || self
                .state
                .borrow()
                .session
                .pane_paths
                .iter()
                .flatten()
                .any(|open| open == path);
        if opened {
            self.close_file(path);
        }
        if let Err(error) = fs::remove_file(path) {
            self.set_status(&format!("Не удалось удалить заметку: {error}"));
            return;
        }
        self.refresh_notes();
        self.set_status("Заметка удалена");
    }

    pub(super) fn move_note_to_workspace(&self, path: &Path) {
        if !path.starts_with(notes::notes_dir()) {
            self.set_status("Это не быстрая заметка");
            return;
        }
        let pins = self.state.borrow().session.pins.clone();
        if pins.is_empty() {
            self.set_status("Сначала запиньте каталог пространства");
            return;
        }
        let dialog = gtk4::Window::builder()
            .transient_for(&self.widgets.window)
            .modal(true)
            .title("Пространство")
            .default_width(440)
            .default_height(320)
            .build();
        dialog.add_css_class("prompt-dialog");
        let hint = gtk4::Label::new(Some("В какое пространство перенести заметку"));
        hint.set_halign(gtk4::Align::Start);
        hint.set_margin_bottom(6);
        let list = gtk4::ListBox::new();
        list.add_css_class("navigation-sidebar");
        list.set_activate_on_single_click(true);
        for pin in &pins {
            let name = pin
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("каталог");
            let title = gtk4::Label::new(Some(name));
            title.set_halign(gtk4::Align::Start);
            title.add_css_class("heading");
            let place = gtk4::Label::new(Some(&pin.display().to_string()));
            place.set_halign(gtk4::Align::Start);
            place.add_css_class("dim-label");
            place.set_ellipsize(gtk4::pango::EllipsizeMode::Middle);
            let row = gtk4::Box::new(gtk4::Orientation::Vertical, 2);
            row.set_margin_top(6);
            row.set_margin_bottom(6);
            row.set_margin_start(8);
            row.set_margin_end(8);
            row.append(&title);
            row.append(&place);
            list.append(&row);
        }
        let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        box_.set_margin_top(12);
        box_.set_margin_bottom(12);
        box_.set_margin_start(12);
        box_.set_margin_end(12);
        box_.append(&hint);
        box_.append(&scrolled_list(&list));
        dialog.set_child(Some(&box_));
        let ctx = self.clone();
        let source = path.to_path_buf();
        let chosen = pins.clone();
        let dialog_close = dialog.clone();
        list.connect_row_activated(move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            let Some(folder) = chosen.get(index as usize).cloned() else {
                return;
            };
            dialog_close.close();
            ctx.prompt_move_destination(&source, &folder);
        });
        dialog.present();
    }

    fn prompt_move_destination(&self, path: &Path, folder: &Path) {
        if !folder.is_dir() {
            self.set_status("Каталог пространства не найден");
            return;
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("заметка.md");
        let dialog = gtk4::FileDialog::builder()
            .title("Перенести заметку")
            .modal(true)
            .accept_label("Перенести")
            .initial_name(name)
            .build();
        dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        let ctx = self.clone();
        let source = path.to_path_buf();
        let window = self.widgets.window.clone();
        dialog.save(
            Some(&window),
            None::<&gio::Cancellable>,
            move |result| {
                if let Ok(file) = result {
                    if let Some(dest) = file.path() {
                        ctx.finish_move_note(&source, &dest);
                    }
                }
            },
        );
    }

    fn finish_move_note(&self, path: &Path, dest: &Path) {
        if same_file(path, dest) {
            self.set_status("Заметка уже лежит по этому пути");
            return;
        }
        let Some(name) = dest.file_name().and_then(|name| name.to_str()) else {
            self.set_status("Введите имя файла");
            return;
        };
        if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
            self.set_status("В имени не должно быть пути");
            return;
        }
        let Some(parent) = dest.parent() else {
            self.set_status("Выберите каталог");
            return;
        };
        if !parent.is_dir() {
            self.set_status("Каталог не найден");
            return;
        }
        if dest.is_dir() {
            self.set_status("Укажите имя файла");
            return;
        }
        let line = self.caret_line(path);
        let open = self.file_is_open(path);
        if open {
            self.close_file(path);
        }
        if self.file_is_open(dest) {
            self.close_file(dest);
        }
        if let Err(error) = move_file(path, dest) {
            self.set_status(&format!("Не удалось перенести заметку: {error}"));
            if path.is_file() && open {
                self.go(path.to_path_buf(), line);
            }
            return;
        }
        self.retarget_paths(path, dest);
        self.go(dest.to_path_buf(), line);
        self.refresh_notes();
        self.mount_workspace();
        self.set_status("Заметка перенесена");
    }

    fn file_is_open(&self, path: &Path) -> bool {
        self.state.borrow().buffers.contains_key(path)
            || self
                .state
                .borrow()
                .session
                .pane_paths
                .iter()
                .flatten()
                .any(|open| open == path)
    }

    fn caret_line(&self, path: &Path) -> Option<u32> {
        self.state
            .borrow()
            .session
            .carets
            .iter()
            .find(|place| place.path == path)
            .map(|place| place.line)
    }

    fn retarget_paths(&self, from: &Path, to: &Path) {
        let mut state = self.state.borrow_mut();
        for path in &mut state.session.open_files {
            if path == from {
                *path = to.to_path_buf();
            }
        }
        for pane in &mut state.session.pane_paths {
            if pane.as_ref() == Some(&from.to_path_buf()) {
                *pane = Some(to.to_path_buf());
            }
        }
        for path in &mut state.session.pinned_files {
            if path == from {
                *path = to.to_path_buf();
            }
        }
        for path in &mut state.session.recent_files {
            if path == from {
                *path = to.to_path_buf();
            }
        }
        for place in &mut state.session.carets {
            if place.path == from {
                place.path = to.to_path_buf();
            }
        }
        for mode in &mut state.session.file_modes {
            if mode.path == from {
                mode.path = to.to_path_buf();
            }
        }
    }

    fn note_text(&self, path: &Path) -> String {
        if let Some(text) = self.buffer_text(path) {
            return text;
        }
        fs::read_to_string(path).unwrap_or_default()
    }
}

struct InboxNote {
    path: PathBuf,
    label: String,
    tags: Vec<String>,
    created: Option<SystemTime>,
    modified: SystemTime,
}

fn note_file_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

fn note_empty_row(filtered: bool) -> gtk4::Label {
    let text = if filtered {
        "Нет заметок с этим тегом"
    } else {
        "Быстрых заметок пока нет"
    };
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(10);
    label.set_margin_top(8);
    label
}

fn clear_note_host(host: &gtk4::Box) {
    while let Some(child) = host.first_child() {
        host.remove(&child);
    }
}

fn sort_notes(notes: &mut [InboxNote], group: notes::NoteGroup) {
    notes.sort_by(|left, right| match group {
        notes::NoteGroup::Created => right
            .created
            .cmp(&left.created)
            .then_with(|| left.label.cmp(&right.label)),
        notes::NoteGroup::Modified | notes::NoteGroup::Flat => right
            .modified
            .cmp(&left.modified)
            .then_with(|| left.label.cmp(&right.label)),
        notes::NoteGroup::Tags => left.label.cmp(&right.label),
    });
}

fn note_stamp(note: &InboxNote, group: notes::NoteGroup) -> (i32, u32, u32) {
    let time = match group {
        notes::NoteGroup::Created => note.created,
        notes::NoteGroup::Modified | notes::NoteGroup::Flat | notes::NoteGroup::Tags => {
            Some(note.modified)
        }
    };
    time.and_then(local_ymd).unwrap_or((0, 0, 0))
}

fn local_ymd(time: SystemTime) -> Option<(i32, u32, u32)> {
    let secs = time.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_secs() as i64;
    let local = gtk4::glib::DateTime::from_unix_local(secs).ok()?;
    Some((local.year(), local.month() as u32, local.day_of_month() as u32))
}

fn month_name(month: u32) -> &'static str {
    match month {
        1 => "январь",
        2 => "февраль",
        3 => "март",
        4 => "апрель",
        5 => "май",
        6 => "июнь",
        7 => "июль",
        8 => "август",
        9 => "сентябрь",
        10 => "октябрь",
        11 => "ноябрь",
        12 => "декабрь",
        _ => "без даты",
    }
}

fn note_branch(title: &str, child: &impl IsA<gtk4::Widget>) -> gtk4::Expander {
    let branch = gtk4::Expander::new(Some(title));
    branch.set_expanded(true);
    branch.set_child(Some(child));
    branch
}

fn scrolled_list(child: &impl IsA<gtk4::Widget>) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .child(child)
        .hexpand(true)
        .vexpand(true)
        .min_content_height(180)
        .build()
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if to.exists() {
        let text = fs::read_to_string(from)?;
        write_atomic(to, &text)?;
        return fs::remove_file(from);
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(18) => {
            fs::copy(from, to)?;
            fs::remove_file(from)
        }
        Err(error) => Err(error),
    }
}
