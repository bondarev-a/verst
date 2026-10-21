use std::path::{Component, Path, PathBuf};

use gio::prelude::ListModelExt;
use gtk4::gio;
use gtk4::prelude::*;
use sourceview5::prelude::{BufferExt, ViewExt};

use text_workspace::session::ViewMode;
use text_workspace::{editor, places, session};

use super::support::{anchor_line_at, buffer_text_of, scroll_top_for_line, scrolled_of};
use super::{Ctx, State};

impl Ctx {
    pub(super) fn preview_focused(&self) -> bool {
        let Some(focus) = gtk4::prelude::RootExt::focus(&self.widgets.window) else {
            return false;
        };
        focus == self.widgets.preview || focus.is_ancestor(&self.widgets.preview)
    }

    pub(super) fn capture_place(&self) -> Option<places::NavPlace> {
        if self.state.borrow().browsing {
            let path = self
                .widgets
                .folder_dir
                .file()
                .and_then(|file| file.path())?;
            return Some(places::NavPlace::Folder { path });
        }
        let path = self.focused_path()?;
        let preview = self.state.borrow().view_mode == ViewMode::Preview;
        let buffer = self.focused_view().buffer();
        let offset = buffer.cursor_position();
        let line = if offset < 0 {
            0
        } else {
            buffer.iter_at_offset(offset).line().max(0) as u32
        };
        Some(places::NavPlace::File {
            path,
            line,
            preview,
        })
    }

    pub(super) fn update_nav_buttons(&self) {
        let state = self.state.borrow();
        self.widgets.back_button.set_sensitive(state.nav.can_back());
        self.widgets
            .forward_button
            .set_sensitive(state.nav.can_forward());
    }

    pub(super) fn go(&self, path: PathBuf, line: Option<u32>) {
        if self.state.borrow().loading {
            self.open_path(path);
            if let Some(line) = line {
                self.jump_to_line(line as usize);
            }
            return;
        }
        let here = self.capture_place();
        let same_file =
            matches!(&here, Some(places::NavPlace::File { path: current, .. }) if current == &path);
        let same_spot = match &here {
            Some(places::NavPlace::File {
                path: current,
                line: at,
                ..
            }) if current == &path => line.is_none_or(|wanted| *at == wanted),
            _ => false,
        };
        if same_spot {
            return;
        }
        let keep_preview = {
            let state = self.state.borrow();
            !state.browsing && state.view_mode == ViewMode::Preview
        };
        if let Some(here) = here {
            self.state.borrow_mut().nav.leave(here);
        }
        if !same_file {
            self.open_file(path, keep_preview);
        }
        if let Some(line) = line {
            self.jump_to_line(line as usize);
        }
        self.update_nav_buttons();
    }

    pub(super) fn shift_history(&self, backward: bool) {
        let here = self.capture_place();
        let dest = {
            let mut state = self.state.borrow_mut();
            if backward {
                state.nav.back(here)
            } else {
                state.nav.forward(here)
            }
        };
        let Some(dest) = dest else {
            self.update_nav_buttons();
            return;
        };
        match dest {
            places::NavPlace::Folder { path } => self.present_folder(path, false),
            places::NavPlace::File {
                path,
                line,
                preview,
            } => {
                let showing =
                    !self.state.borrow().browsing && self.focused_path().as_ref() == Some(&path);
                if !showing {
                    self.open_file(path, true);
                } else {
                    self.state.borrow_mut().browsing = false;
                }
                let mode = if preview {
                    ViewMode::Preview
                } else {
                    ViewMode::Edit
                };
                self.state.borrow_mut().view_mode = mode;
                if let Some(path) = self.focused_path() {
                    self.state
                        .borrow_mut()
                        .session
                        .remember_mode(path, mode);
                }
                self.jump_to_line(line as usize);
                self.apply_view_mode();
                self.update_nav_buttons();
            }
        }
    }

    pub(super) fn focused_view(&self) -> sourceview5::View {
        self.widgets.views[self.focused_index()].clone()
    }

    pub(super) fn edit_undo(&self) {
        self.focused_view().buffer().undo();
    }

    pub(super) fn edit_redo(&self) {
        self.focused_view().buffer().redo();
    }

    pub(super) fn edit_select_all(&self) {
        let buffer = self.focused_view().buffer();
        let start = buffer.start_iter();
        let end = buffer.end_iter();
        buffer.select_range(&start, &end);
        self.focused_view().grab_focus();
    }

    pub(super) fn jump_to_line(&self, line: usize) {
        let pane = self.focused_index();
        let view = self.widgets.views[pane].clone();
        let buffer = view.buffer();
        let Some(mut iter) = buffer.iter_at_line(line as i32) else {
            return;
        };
        buffer.place_cursor(&iter);
        view.scroll_to_iter(&mut iter, 0.1, true, 0.0, 0.3);
        view.grab_focus();
    }

    pub(super) fn editor_anchor_line(&self) -> Option<u32> {
        let view = self.focused_view();
        let scroll = scrolled_of(&view)?;
        let adj = scroll.vadjustment();
        if adj.page_size() < 1.0 {
            return None;
        }
        let y = (adj.value() + adj.page_size() / 3.0).round() as i32;
        let (iter, _) = gtk4::prelude::TextViewExt::line_at_y(&view, y);
        let line = iter.line().max(0);
        let next_top = view
            .buffer()
            .iter_at_line(line + 1)
            .map(|next| gtk4::prelude::TextViewExt::line_yrange(&view, &next).0);
        Some(anchor_line_at(y, line, next_top).max(0) as u32)
    }

    pub(super) fn scroll_editor_to_line(&self, line: u32) -> bool {
        let view = self.focused_view();
        let Some(scroll) = scrolled_of(&view) else {
            return false;
        };
        let adj = scroll.vadjustment();
        if adj.page_size() < 1.0 {
            return false;
        }
        let Some(iter) = view.buffer().iter_at_line(line as i32) else {
            return true;
        };
        let (top, _) = gtk4::prelude::TextViewExt::line_yrange(&view, &iter);
        if line > 0 && top == 0 {
            return false;
        }
        adj.set_value(scroll_top_for_line(top as f64, adj.page_size(), adj.upper()));
        true
    }

    pub(super) fn set_theme(&self, theme: &str) {
        if self.state.borrow().session.theme == theme {
            return;
        }
        self.state.borrow_mut().session.theme = theme.to_string();
        let buffers: Vec<sourceview5::Buffer> = self
            .state
            .borrow()
            .buffers
            .values()
            .map(|entry| entry.buffer.clone())
            .collect();
        for buffer in buffers {
            self.apply_scheme(&buffer);
        }
        self.apply_scheme(&self.widgets.empty_buffer);
        self.save_session();
    }

    pub(super) fn apply_scheme(&self, buffer: &sourceview5::Buffer) {
        let theme = self.state.borrow().session.theme.clone();
        let scheme = sourceview5::StyleSchemeManager::default().scheme(&theme);
        buffer.set_style_scheme(scheme.as_ref());
    }

    pub(super) fn apply_language(&self, buffer: &sourceview5::Buffer, path: &Path) {
        let language = sourceview5::LanguageManager::default().guess_language(Some(path), None);
        buffer.set_language(language.as_ref());
    }

    pub(super) fn apply_profile(&self, view: &sourceview5::View) {
        let language_id = view
            .buffer()
            .downcast::<sourceview5::Buffer>()
            .ok()
            .and_then(|buffer| buffer.language())
            .map(|language| language.id().to_string());
        let profile = editor::profile_for(language_id.as_deref());
        view.set_show_line_numbers(true);
        view.set_highlight_current_line(true);
        view.set_tab_width(profile.tab_width);
        view.set_insert_spaces_instead_of_tabs(profile.insert_spaces);
        view.set_wrap_mode(self.wrap_mode());
    }

    pub(super) fn wrap_mode(&self) -> gtk4::WrapMode {
        if self.state.borrow().session.word_wrap {
            gtk4::WrapMode::WordChar
        } else {
            gtk4::WrapMode::None
        }
    }

    pub(super) fn apply_wrap(&self) {
        let mode = self.wrap_mode();
        for view in &self.widgets.views {
            view.set_wrap_mode(mode);
        }
    }

    pub(super) fn capture_carets(&self, state: &mut State) {
        let mut carets = Vec::new();
        for pane in 0..2 {
            let Some(path) = state.session.pane_paths.get(pane).cloned().flatten() else {
                continue;
            };
            let buffer = self.widgets.views[pane].buffer();
            let offset = buffer.cursor_position();
            let iter = if offset < 0 {
                buffer.start_iter()
            } else {
                buffer.iter_at_offset(offset)
            };
            carets.push(session::CaretPlace {
                path,
                line: iter.line().max(0) as u32,
                column: iter.line_offset().max(0) as u32,
            });
        }
        state.session.carets = carets;
    }

    pub(super) fn restore_carets(&self, session: &session::Session) {
        let focused = session.focused_pane.min(1) as usize;
        for (pane, path) in session.pane_paths.iter().enumerate() {
            let Some(path) = path else {
                continue;
            };
            let Some(place) = session.carets.iter().find(|place| &place.path == path) else {
                continue;
            };
            self.place_cursor(pane, place.line, place.column, pane == focused);
        }
    }

    pub(super) fn place_cursor(&self, pane: usize, line: u32, column: u32, focus: bool) {
        let Some(view) = self.widgets.views.get(pane) else {
            return;
        };
        let buffer = view.buffer();
        let Some(mut iter) = buffer.iter_at_line(line as i32) else {
            return;
        };
        if column > 0 {
            iter.forward_chars(column as i32);
        }
        buffer.place_cursor(&iter);
        view.scroll_to_iter(&mut iter, 0.1, true, 0.0, 0.3);
        if focus {
            view.grab_focus();
        }
    }

    pub(super) fn focused_index(&self) -> usize {
        let state = self.state.borrow();
        if state.session.split {
            state.session.focused_pane.min(1)
        } else {
            0
        }
    }

    pub(super) fn focused_path(&self) -> Option<PathBuf> {
        let pane = self.focused_index();
        self.state
            .borrow()
            .session
            .pane_paths
            .get(pane)
            .cloned()
            .flatten()
    }

    pub(super) fn buffer_text(&self, path: &Path) -> Option<String> {
        self.state
            .borrow()
            .buffers
            .get(path)
            .map(|entry| buffer_text_of(&entry.buffer))
    }

    pub(super) fn pin_path(&self) -> Option<PathBuf> {
        let state = self.state.borrow();
        state
            .session
            .active_pin_path()
            .map(|path| path.to_path_buf())
    }

    pub(super) fn browse_focused(&self) {
        if let Some(path) = self.focused_path() {
            self.show_folder(path);
            return;
        }
        if let Some(pin) = self.pin_path() {
            self.show_folder(pin);
            return;
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        self.show_folder(home);
    }

    pub(super) fn show_folder(&self, path: PathBuf) {
        self.present_folder(path, true);
    }

    pub(super) fn present_folder(&self, path: PathBuf, record: bool) {
        let dir = if path.is_dir() {
            path
        } else {
            let Some(parent) = path.parent() else {
                self.set_status("Это не каталог");
                return;
            };
            if parent.as_os_str().is_empty() {
                self.set_status("Это не каталог");
                return;
            }
            parent.to_path_buf()
        };
        if !dir.is_dir() {
            self.set_status("Каталог недоступен");
            return;
        }
        if record && !self.state.borrow().loading {
            let here = self.capture_place();
            let same = matches!(&here, Some(places::NavPlace::Folder { path }) if path == &dir);
            if !same {
                if let Some(here) = here {
                    self.state.borrow_mut().nav.leave(here);
                }
            }
        }
        self.widgets
            .folder_dir
            .set_file(Some(&gio::File::for_path(&dir)));
        self.fill_crumbs(&dir);
        let can_up = dir
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty() && parent != dir);
        self.widgets.folder_up.set_sensitive(can_up);
        self.state.borrow_mut().browsing = true;
        self.apply_view_mode();
        self.widgets.folder_view.grab_focus();
        self.update_nav_buttons();
    }

    pub(super) fn folder_up(&self) {
        let Some(dir) = self.widgets.folder_dir.file().and_then(|file| file.path()) else {
            return;
        };
        let Some(parent) = dir.parent() else {
            return;
        };
        if parent.as_os_str().is_empty() || parent == dir {
            return;
        }
        self.show_folder(parent.to_path_buf());
    }

    pub(super) fn open_folder_item(&self, position: u32) {
        let Some(info) = self
            .widgets
            .folder_view
            .model()
            .and_then(|model| model.item(position))
            .and_then(|item| item.downcast::<gio::FileInfo>().ok())
        else {
            return;
        };
        let Some(path) = info
            .attribute_object("standard::file")
            .and_then(|file| file.downcast::<gio::File>().ok())
            .and_then(|file| file.path())
        else {
            return;
        };
        if path.is_dir() {
            self.show_folder(path);
            return;
        }
        self.go(path, None);
    }

    pub(super) fn fill_crumbs(&self, dir: &Path) {
        let crumbs = &self.widgets.folder_crumbs;
        while let Some(child) = crumbs.first_child() {
            crumbs.remove(&child);
        }
        let mut acc = PathBuf::new();
        let mut first = true;
        for component in dir.components() {
            let label = match component {
                Component::RootDir => "Корень".to_string(),
                Component::Normal(name) => name.to_string_lossy().into_owned(),
                _ => continue,
            };
            acc.push(component);
            if !first {
                let sep = gtk4::Label::new(Some("›"));
                sep.add_css_class("dim-label");
                sep.set_margin_start(2);
                sep.set_margin_end(2);
                crumbs.append(&sep);
            }
            first = false;
            let button = gtk4::Button::with_label(&label);
            button.add_css_class("flat");
            button.add_css_class("crumb");
            let target = acc.clone();
            let ctx = self.clone();
            button.connect_clicked(move |_| ctx.show_folder(target.clone()));
            crumbs.append(&button);
        }
    }
}
