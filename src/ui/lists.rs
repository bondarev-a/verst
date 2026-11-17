//! Списки «Открытые файлы» и «История».
//!
//! Строки создаются один раз. При переходе между файлами меняются только
//! подсветка текущего файла, точка ● у несохранённых и булавка. Список
//! целиком пересобирается, лишь когда меняется его состав.

use std::path::{Path, PathBuf};

use gtk4::prelude::*;
use text_workspace::places;

use super::support::{clear_box, clear_list, dim_row, file_icon, file_label};
use super::Ctx;

const CURRENT: &str = "suggested-action";

struct OpenChip {
    path: PathBuf,
    button: gtk4::Button,
    label: gtk4::Label,
}

struct HistoryRow {
    path: PathBuf,
    row: gtk4::ListBoxRow,
    label: gtk4::Label,
    pin: gtk4::Button,
}

#[derive(Default)]
pub(super) struct ListRows {
    chips: Vec<OpenChip>,
    history: Vec<HistoryRow>,
    /// В «Истории» сейчас строка «История пуста», а не файлы.
    history_built: bool,
}

impl Ctx {
    pub(super) fn refresh_open_list(&self) {
        let _prof = crate::ui::prof::span("refresh_open_list");
        self.sync_open_chips();
        self.sync_history_rows();
    }

    fn title_of(&self, path: &Path) -> String {
        let dirty = self
            .state
            .borrow()
            .buffers
            .get(path)
            .is_some_and(|entry| entry.dirty);
        let name = file_label(path);
        if dirty {
            format!("● {name}")
        } else {
            name
        }
    }

    fn sync_open_chips(&self) {
        let same = {
            let state = self.state.borrow();
            let files = &state.session.open_files;
            let chips = &state.lists.chips;
            // Полоса показывает файлы от новых к старым.
            files.len() == chips.len()
                && files.iter().rev().zip(chips).all(|(path, chip)| *path == chip.path)
        };
        if !same {
            self.rebuild_pins();
        }
        self.mark_open_chips();
    }

    fn mark_open_chips(&self) {
        let focused = self.focused_path();
        let marks: Vec<(gtk4::Button, gtk4::Label, PathBuf)> = self
            .state
            .borrow()
            .lists
            .chips
            .iter()
            .map(|chip| (chip.button.clone(), chip.label.clone(), chip.path.clone()))
            .collect();
        for (button, label, path) in marks {
            set_class(&button, CURRENT, focused.as_ref() == Some(&path));
            set_label(&label, &self.title_of(&path));
        }
    }

    pub(super) fn rebuild_pins(&self) {
        let _prof = crate::ui::prof::span("rebuild_pins");
        clear_box(&self.widgets.pins_box);
        self.state.borrow_mut().lists.chips.clear();
        let files = self.state.borrow().session.open_files.clone();
        if files.is_empty() {
            let hint = gtk4::Label::new(Some("Нет открытых файлов"));
            hint.add_css_class("dim-label");
            hint.set_margin_start(4);
            self.widgets.pins_box.append(&hint);
            return;
        }
        let mut chips = Vec::with_capacity(files.len());
        for path in files.iter().rev() {
            let label = gtk4::Label::new(Some(&self.title_of(path)));
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(28);
            label.set_can_target(false);
            let button = gtk4::Button::new();
            button.set_child(Some(&label));
            button.add_css_class("pin-name");
            button.set_tooltip_text(Some(&path.display().to_string()));
            let ctx = self.clone();
            let open_path = path.clone();
            button.connect_clicked(move |_| ctx.go(open_path.clone(), None));
            let close = gtk4::Button::from_icon_name("window-close-symbolic");
            close.add_css_class("flat");
            close.add_css_class("pin-close");
            close.set_tooltip_text(Some("Закрыть файл"));
            let ctx = self.clone();
            let close_path = path.clone();
            close.connect_clicked(move |_| ctx.close_file(&close_path));
            close.set_has_frame(false);
            close.set_valign(gtk4::Align::Center);
            let group = gtk4::Box::new(gtk4::Orientation::Horizontal, 2);
            group.add_css_class("open-chip");
            group.append(&button);
            group.append(&close);
            self.widgets.pins_box.append(&group);
            chips.push(OpenChip {
                path: path.clone(),
                button,
                label,
            });
        }
        self.state.borrow_mut().lists.chips = chips;
    }

    fn sync_history_rows(&self) {
        let files = self.state.borrow().session.recent_files.clone();
        let (built, current): (bool, Vec<PathBuf>) = {
            let state = self.state.borrow();
            (
                state.lists.history_built,
                state.lists.history.iter().map(|row| row.path.clone()).collect(),
            )
        };
        if !built || files.is_empty() || current.is_empty() {
            self.rebuild_history();
        } else if current != files && !self.shift_history_rows(&current, &files) {
            self.rebuild_history();
        }
        self.mark_history_rows();
    }

    /// Новый порядок отличается от старого одним файлом, поднятым наверх
    /// (уже был в списке или открыт впервые). Двигаем одну строку.
    fn shift_history_rows(&self, current: &[PathBuf], files: &[PathBuf]) -> bool {
        let top = &files[0];
        let mut expected: Vec<PathBuf> = current.iter().filter(|p| *p != top).cloned().collect();
        expected.insert(0, top.clone());
        expected.truncate(places::RECENT_CAP);
        if expected != files {
            return false;
        }
        let list = &self.widgets.history_list;
        let position = current.iter().position(|p| p == top);
        let moved = match position {
            Some(index) => {
                let row = self.state.borrow_mut().lists.history.remove(index);
                list.remove(&row.row);
                row
            }
            None => self.history_row(top),
        };
        list.insert(&moved.row, 0);
        let extras: Vec<HistoryRow> = {
            let mut state = self.state.borrow_mut();
            let rows = &mut state.lists.history;
            rows.insert(0, moved);
            let keep = files.len().min(rows.len());
            rows.split_off(keep)
        };
        for extra in extras {
            list.remove(&extra.row);
        }
        true
    }

    fn rebuild_history(&self) {
        let _prof = crate::ui::prof::span("refresh_history");
        clear_list(&self.widgets.history_list);
        self.state.borrow_mut().lists.history.clear();
        let files = self.state.borrow().session.recent_files.clone();
        if files.is_empty() {
            self.widgets.history_list.append(&dim_row("История пуста"));
            self.state.borrow_mut().lists.history_built = false;
            return;
        }
        let rows: Vec<HistoryRow> = files.iter().map(|path| self.history_row(path)).collect();
        for row in &rows {
            self.widgets.history_list.append(&row.row);
        }
        let mut state = self.state.borrow_mut();
        state.lists.history = rows;
        state.lists.history_built = true;
    }

    fn mark_history_rows(&self) {
        let focused = self.focused_path();
        let marks: Vec<(gtk4::Label, gtk4::Button, PathBuf)> = self
            .state
            .borrow()
            .lists
            .history
            .iter()
            .map(|row| (row.label.clone(), row.pin.clone(), row.path.clone()))
            .collect();
        for (label, pin, path) in marks {
            set_class(&label, "heading", focused.as_ref() == Some(&path));
            set_label(&label, &self.title_of(&path));
            let pinned = self
                .state
                .borrow()
                .session
                .pinned_files
                .iter()
                .any(|item| item == &path);
            set_class(&pin, CURRENT, pinned);
            pin.set_tooltip_text(Some(if pinned {
                "Открепить файл"
            } else {
                "Закрепить файл"
            }));
        }
    }

    fn history_row(&self, path: &Path) -> HistoryRow {
        let label = gtk4::Label::new(Some(&self.title_of(path)));
        label.set_halign(gtk4::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        label.set_can_target(false);
        let pin = gtk4::Button::from_icon_name("view-pin-symbolic");
        pin.add_css_class("flat");
        pin.add_css_class("row-action");
        let ctx = self.clone();
        let watch = path.to_path_buf();
        pin.connect_clicked(move |_| ctx.toggle_pin_file(&watch));
        let line = gtk4::Box::new(gtk4::Orientation::Horizontal, 6);
        line.add_css_class("file-row");
        line.set_tooltip_text(Some(&path.display().to_string()));
        let icon = file_icon(path);
        icon.set_can_target(false);
        line.append(&icon);
        line.append(&label);
        line.append(&pin);
        let row = gtk4::ListBoxRow::new();
        row.set_child(Some(&line));
        HistoryRow {
            path: path.to_path_buf(),
            row,
            label,
            pin,
        }
    }
}

impl Ctx {
    /// Для замеров: строки списков совпадают с сессией, подсвечен ровно текущий файл.
    pub(super) fn lists_report(&self) -> String {
        let focused = self.focused_path();
        let state = self.state.borrow();
        let mut problems = Vec::new();
        let open: Vec<&PathBuf> = state.session.open_files.iter().rev().collect();
        let chips: Vec<&PathBuf> = state.lists.chips.iter().map(|c| &c.path).collect();
        if open != chips {
            problems.push("порядок открытых".to_string());
        }
        let lit: Vec<&PathBuf> = state
            .lists
            .chips
            .iter()
            .filter(|c| c.button.has_css_class(CURRENT))
            .map(|c| &c.path)
            .collect();
        if lit.len() != 1 || Some(lit[0]) != focused.as_ref() {
            problems.push(format!("подсветка открытых: {}", lit.len()));
        }
        let rows: Vec<&PathBuf> = state.lists.history.iter().map(|r| &r.path).collect();
        let recent: Vec<&PathBuf> = state.session.recent_files.iter().collect();
        if rows != recent {
            problems.push("порядок истории".to_string());
        }
        for (index, row) in state.lists.history.iter().enumerate() {
            if row.row.index() != index as i32 {
                problems.push(format!("строка {index} на месте {}", row.row.index()));
                break;
            }
        }
        let bold = state
            .lists
            .history
            .iter()
            .filter(|r| r.label.has_css_class("heading"))
            .count();
        if bold != 1 {
            problems.push(format!("подсветка истории: {bold}"));
        }
        if problems.is_empty() {
            "ok".to_string()
        } else {
            problems.join(", ")
        }
    }
}

fn set_class(widget: &impl IsA<gtk4::Widget>, class: &str, on: bool) {
    if widget.has_css_class(class) != on {
        if on {
            widget.add_css_class(class);
        } else {
            widget.remove_css_class(class);
        }
    }
}

fn set_label(label: &gtk4::Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}
