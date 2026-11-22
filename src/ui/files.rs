use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk4::prelude::*;
use gtk4::{gio, glib};
use text_workspace::session::ViewMode;
use text_workspace::{kind, notes, places};

use super::support::{buffer_text_of, read_text, watch_file, write_atomic};
use super::{BufferEntry, Ctx, AUTOSAVE_MS};

impl Ctx {
    pub(super) fn open_path(&self, path: PathBuf) {
        self.open_file(path, false);
    }

    pub(super) fn open_file(&self, path: PathBuf, keep_mode: bool) {
    let _prof = crate::ui::prof::span("open_file");
        if path.is_dir() {
            return;
        }
        // Активная ячейка таблицы принадлежит уходящему файлу. Пока он ещё в
        // фокусе, записать её в свой файл и снять блокировку ввода; иначе новый
        // файл оставался только для чтения, а набранное в ячейке терялось.
        if self.focused_path().as_deref() != Some(path.as_path()) && self.any_table_cell_focused() {
            self.deactivate_table_cells();
        }
        let text_file = kind::is_text(&path);
        if text_file {
            if let Err(message) = self.ensure_buffer(&path) {
                self.set_status(&message);
                return;
            }
        }
        let pane = self.focused_index();
        self.stash_pane_caret(pane);
        {
            let mut state = self.state.borrow_mut();
            state.browsing = false;
            if notes::is_inbox(&path) || !state.loading {
                let mode = if notes::is_inbox(&path) {
                    ViewMode::Edit
                } else if keep_mode {
                    state.session.mode_of(&path).unwrap_or(state.view_mode)
                } else {
                    state
                        .session
                        .mode_of(&path)
                        .unwrap_or(state.session.open_mode)
                };
                state.view_mode = mode;
                state.session.view_mode = mode;
                state.session.remember_mode(path.clone(), mode);
            }
            text_workspace::session::remember_open(&mut state.session.open_files, path.clone());
            if state.session.pane_paths.len() < 2 {
                state.session.pane_paths.resize(2, None);
            }
            state.session.pane_paths[pane] = Some(path.clone());
            if !state.loading {
                places::remember(&mut state.session.recent_files, path.clone());
            }
        }
        let buffer = self
            .state
            .borrow()
            .buffers
            .get(&path)
            .map(|entry| entry.buffer.clone());
        if let Some(buffer) = buffer {
            self.widgets.views[pane].set_buffer(Some(&buffer));
            self.apply_profile(&self.widgets.views[pane]);
            self.widgets.views[pane].queue_allocate();
        } else if !text_file {
            self.widgets.views[pane].set_buffer(Some(&self.widgets.empty_buffer));
        }
        // Списки обновляет follow_focus: второй проход здесь был лишним.
        self.follow_focus();
        self.settle_open_place(pane, &path, false);
        let ctx = self.clone();
        let watch = path.clone();
        glib::idle_add_local_once(move || {
            let current = ctx
                .state
                .borrow()
                .session
                .pane_paths
                .get(pane)
                .cloned()
                .flatten();
            if current.as_deref() != Some(watch.as_path()) {
                return;
            }
            ctx.settle_open_place(pane, &watch, true);
        });
        if !self.state.borrow().loading {
            self.enforce_open_limit();
        }
        self.save_session();
    }

    pub(super) fn enforce_open_limit(&self) {
        let extra = {
            let state = self.state.borrow();
            let limit = state.session.open_limit as usize;
            let panes: Vec<PathBuf> = state.session.pane_paths.iter().flatten().cloned().collect();
            let overflow = state.session.open_files.len().saturating_sub(limit);
            state
                .session
                .open_files
                .iter()
                .filter(|open| !panes.iter().any(|pane| pane == *open))
                .take(overflow)
                .cloned()
                .collect::<Vec<_>>()
        };
        for path in extra {
            self.close_file(&path);
        }
    }

    /// Закрыть файл. Изменения сначала записываются; если записать не
    /// удалось, файл остаётся открытым и возвращается `false`.
    pub(super) fn close_file(&self, path: &Path) -> bool {
    let _prof = crate::ui::prof::span("close_file");
        if self.focused_path().as_deref() == Some(path) && self.any_table_cell_focused() {
            self.deactivate_table_cells();
        }
        if self
            .state
            .borrow()
            .buffers
            .get(path)
            .is_some_and(|entry| entry.dirty)
            && !self.write_file(path, false)
        {
            self.set_status(&format!(
                "Файл не закрыт: не удалось записать {}",
                path.display()
            ));
            return false;
        }
        self.drop_render_for_path(path);
        for pane in 0..2 {
            let shows = self
                .state
                .borrow()
                .session
                .pane_paths
                .get(pane)
                .cloned()
                .flatten();
            if shows.as_deref() == Some(path) {
                self.widgets.views[pane].set_buffer(Some(&self.widgets.empty_buffer));
            }
        }
        {
            let mut state = self.state.borrow_mut();
            state.session.open_files.retain(|open| open != path);
            state.session.carets.retain(|place| place.path != path);
            for pane in &mut state.session.pane_paths {
                if pane.as_ref().is_some_and(|open| open == path) {
                    *pane = None;
                }
            }
            if let Some(id) = state.autosave.remove(path) {
                id.remove();
            }
            state.buffers.remove(path);
        }
        self.refresh_open_list();
        self.follow_focus();
        self.save_session();
        true
    }

    pub(super) fn ensure_buffer(&self, path: &Path) -> Result<(), String> {
    let _prof = crate::ui::prof::span("ensure_buffer");
        if self.state.borrow().buffers.contains_key(path) {
            return Ok(());
        }
        let text = read_text(path)?;
        let buffer = sourceview5::Buffer::new(None);
        buffer.set_text(&text);
        buffer.set_enable_undo(true);
        self.apply_language(&buffer, path);
        self.apply_scheme(&buffer);
        let ctx = self.clone();
        let identity = Rc::new(RefCell::new(path.to_path_buf()));
        let watch_id = identity.clone();
        // Счётчик правок текста. Кеши ссылок и тегов сверяются с ним. Вставка
        // и удаление якорей таблиц (U+FFFC) текст файла не меняют и не считаются.
        let rev = Rc::new(std::cell::Cell::new(0u64));
        let bump = rev.clone();
        buffer.connect_insert_text(move |_, _, _| bump.set(bump.get().wrapping_add(1)));
        let bump = rev.clone();
        buffer.connect_delete_range(move |_, start, end| {
            if start.slice(end).chars().any(|c| c != '\u{FFFC}') {
                bump.set(bump.get().wrapping_add(1));
            }
        });
        buffer.connect_changed(move |_| {
            if ctx.state.borrow().loading || ctx.state.borrow().render.writing {
                return;
            }
            let watch = watch_id.borrow().clone();
            ctx.note_text_edit(&watch, true);
        });
        let ctx = self.clone();
        buffer.connect_mark_set(move |_, _, mark| {
            let name = mark.name().map(|name| name.to_string()).unwrap_or_default();
            if name == "insert" || name == "selection_bound" {
                ctx.sync_ask_chip();
            }
        });
        let monitor = watch_file(path, self.clone());
        self.state.borrow_mut().buffers.insert(
            path.to_path_buf(),
            BufferEntry {
                buffer,
                dirty: false,
                last_saved: text,
                acked_external: None,
                identity,
                monitor,
                render_hooked: false,
                rev,
                links_synced: None,
                tags_synced: None,
                outgoing_cache: None,
            },
        );
        self.apply_text_zoom_tags();
        Ok(())
    }

    pub(super) fn save_focused(&self) {
        let Some(path) = self.focused_path() else {
            return;
        };
        self.write_file(&path, false);
    }

    pub(super) fn save_as(&self) {
        let Some(source) = self.focused_path() else {
            self.set_status("Нет открытого файла");
            return;
        };
        if !kind::is_text(&source) {
            self.set_status("Сохранить как записывает текстовый файл");
            return;
        }
        if self.buffer_text(&source).is_none() {
            self.set_status("Файл ещё не прочитан");
            return;
        }
        let parent = source
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("file.txt");
        let dialog = gtk4::FileDialog::builder()
            .title("Сохранить как")
            .modal(true)
            .accept_label("Сохранить")
            .initial_name(name)
            .build();
        dialog.set_initial_folder(Some(&gio::File::for_path(&parent)));
        let ctx = self.clone();
        let window = self.widgets.window.clone();
        dialog.save(Some(&window), None::<&gio::Cancellable>, move |result| {
            let Ok(file) = result else {
                return;
            };
            let Some(dest) = file.path() else {
                return;
            };
            ctx.finish_save_as(&source, dest);
        });
    }

    fn finish_save_as(&self, source: &Path, dest: PathBuf) {
        if dest.is_dir() {
            self.set_status("Выберите файл");
            return;
        }
        if same_file(source, &dest) {
            self.write_file(source, false);
            return;
        }
        if self
            .state
            .borrow()
            .buffers
            .get(&dest)
            .is_some_and(|entry| entry.dirty)
        {
            self.set_status("Файл с этим именем открыт и не сохранён");
            return;
        }
        let Some(text) = self.buffer_text(source) else {
            self.set_status("Файл ещё не прочитан");
            return;
        };
        if let Some(parent) = dest.parent() {
            if !parent.as_os_str().is_empty() && !parent.is_dir() {
                self.set_status("Каталог для файла не найден");
                return;
            }
        }
        if let Err(error) = write_atomic(&dest, &text) {
            self.set_status(&format!("Не удалось записать файл: {error}"));
            return;
        }
        self.adopt_saved_path(source, &dest, &text);
    }

    fn adopt_saved_path(&self, source: &Path, dest: &Path, text: &str) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(id) = state.autosave.remove(source) {
                id.remove();
            }
            if let Some(id) = state.autosave.remove(dest) {
                id.remove();
            }
        }
        let Some(mut entry) = self.state.borrow_mut().buffers.remove(source) else {
            self.open_file(dest.to_path_buf(), true);
            self.set_status(&format!("Сохранено как {}", dest.display()));
            return;
        };
        self.state.borrow_mut().buffers.remove(dest);
        *entry.identity.borrow_mut() = dest.to_path_buf();
        entry.links_synced = None;
        entry.tags_synced = None;
        entry.outgoing_cache = None;
        entry.dirty = false;
        entry.last_saved = text.to_string();
        entry.acked_external = None;
        entry.monitor = watch_file(dest, self.clone());
        self.apply_language(&entry.buffer, dest);
        let buffer = entry.buffer.clone();
        self.state
            .borrow_mut()
            .buffers
            .insert(dest.to_path_buf(), entry);
        let mode = self.state.borrow().view_mode;
        {
            let mut state = self.state.borrow_mut();
            let dest_buf = dest.to_path_buf();
            let files = &mut state.session.open_files;
            if let Some(index) = files.iter().position(|open| open == source) {
                files[index] = dest_buf.clone();
            }
            let mut kept_dest = false;
            files.retain(|open| {
                if open == source {
                    return false;
                }
                if open == &dest_buf {
                    if kept_dest {
                        return false;
                    }
                    kept_dest = true;
                }
                true
            });
            if !kept_dest {
                files.push(dest_buf);
            }
            for pane in &mut state.session.pane_paths {
                if pane.as_deref() == Some(source) || pane.as_deref() == Some(dest) {
                    *pane = Some(dest.to_path_buf());
                }
            }
            for place in &mut state.session.carets {
                if place.path == source {
                    place.path = dest.to_path_buf();
                }
            }
            state.session.remember_mode(dest.to_path_buf(), mode);
            places::remember(&mut state.session.recent_files, dest.to_path_buf());
        }
        for pane in 0..2 {
            let shows = self
                .state
                .borrow()
                .session
                .pane_paths
                .get(pane)
                .cloned()
                .flatten();
            if shows.as_deref() == Some(dest) {
                self.widgets.views[pane].set_buffer(Some(&buffer));
                self.apply_profile(&self.widgets.views[pane]);
            }
        }
        if source.starts_with(notes::notes_dir()) || dest.starts_with(notes::notes_dir()) {
            self.refresh_notes();
        }
        self.follow_focus();
        self.save_session();
        self.set_status(&format!("Сохранено как {}", dest.display()));
    }

    /// Текст файла изменён: правкой пользователя или правкой таблицы.
    /// Правки таблиц идут под флагом `render.writing`, и обработчик
    /// `changed` их пропускает, поэтому они вызывают это напрямую.
    /// `rerender = false` — не перерисовывать документ (активна ячейка).
    pub(super) fn note_text_edit(&self, path: &Path, rerender: bool) {
        let became_dirty = self
            .state
            .borrow_mut()
            .buffers
            .get_mut(path)
            .is_some_and(|entry| {
                let was = entry.dirty;
                entry.dirty = true;
                entry.acked_external = None;
                !was
            });
        self.schedule_autosave(path.to_path_buf());
        if became_dirty {
            self.refresh_open_list();
        }
        if rerender {
            self.schedule_preview();
        } else {
            self.schedule_side_and_preview();
        }
    }

    pub(super) fn schedule_autosave(&self, path: PathBuf) {
        // Пока пользователь решает, чья версия файла верна, не пишем его.
        if self.state.borrow().conflict.as_ref() == Some(&path) {
            return;
        }
        if let Some(id) = self.state.borrow_mut().autosave.remove(&path) {
            id.remove();
        }
        let ctx = self.clone();
        let watch = path.clone();
        let id = glib::timeout_add_local(Duration::from_millis(AUTOSAVE_MS), move || {
            ctx.state.borrow_mut().autosave.remove(&watch);
            ctx.write_file(&watch, true);
            glib::ControlFlow::Break
        });
        self.state.borrow_mut().autosave.insert(path, id);
    }

    /// Записать буфер в файл. `false` — запись не удалась, текст есть только в памяти.
    pub(super) fn write_file(&self, path: &Path, from_autosave: bool) -> bool {
    let _prof = crate::ui::prof::span("write_file");
        let Some(text) = self.buffer_text(path) else {
            return true;
        };
        let dirty = self
            .state
            .borrow()
            .buffers
            .get(path)
            .is_some_and(|entry| entry.dirty);
        if from_autosave && !dirty {
            return true;
        }
        if from_autosave && self.state.borrow().conflict.as_deref() == Some(path) {
            return true;
        }
        match write_atomic(path, &text) {
            Ok(()) => {
                if let Some(entry) = self.state.borrow_mut().buffers.get_mut(path) {
                    // Только после успешной записи: иначе сверка с диском
                    // считала бы несохранённый текст сохранённым.
                    entry.last_saved = text;
                    entry.dirty = false;
                    entry.acked_external = None;
                }
                self.refresh_open_list();
                self.refresh_file_facts();
                if path.starts_with(text_workspace::notes::notes_dir()) {
                    self.refresh_notes();
                }
                if from_autosave {
                    self.set_status("Автосохранение");
                } else {
                    self.set_status("Сохранено");
                }
                true
            }
            Err(error) => {
                self.set_status(&format!("Не удалось записать файл: {error}"));
                false
            }
        }
    }

    /// Записать все изменённые файлы. Возвращает те, что записать не удалось.
    pub(super) fn write_all_dirty(&self) -> Vec<PathBuf> {
        if self.any_table_cell_focused() {
            self.deactivate_table_cells();
        }
        let dirty: Vec<PathBuf> = self
            .state
            .borrow()
            .buffers
            .iter()
            .filter(|(_, entry)| entry.dirty)
            .map(|(path, _)| path.clone())
            .collect();
        dirty
            .into_iter()
            .filter(|path| !self.write_file(path, false))
            .collect()
    }

    pub(super) fn on_disk_changed(&self, path: &Path) {
    let _prof = crate::ui::prof::span("on_disk_changed");
        if self.state.borrow().conflict.is_some() {
            let mut state = self.state.borrow_mut();
            if !state.conflict_queue.iter().any(|queued| queued == path) {
                state.conflict_queue.push(path.to_path_buf());
            }
            return;
        }
        let Ok(disk) = read_text(path) else {
            return;
        };
        let Some((buffer_text, last_saved, acked)) =
            self.state.borrow().buffers.get(path).map(|entry| {
                (
                    buffer_text_of(&entry.buffer),
                    entry.last_saved.clone(),
                    entry.acked_external.clone(),
                )
            })
        else {
            return;
        };
        if disk == buffer_text || disk == last_saved || acked.as_ref() == Some(&disk) {
            return;
        }
        // Своих правок в буфере нет — просто показываем версию с диска.
        if buffer_text == last_saved {
            self.reload(path, &disk);
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.conflict = Some(path.to_path_buf());
            if let Some(id) = state.autosave.remove(path) {
                id.remove();
            }
        }
        let dialog = gtk4::AlertDialog::builder()
            .modal(true)
            .message("Файл изменён на диске")
            .detail(path.display().to_string())
            .buttons(["Оставить в редакторе", "Перечитать с диска"])
            .default_button(0)
            .cancel_button(0)
            .build();
        let ctx = self.clone();
        let watch = path.to_path_buf();
        let disk_copy = disk.clone();
        let window = self.widgets.window.clone();
        dialog.choose(Some(&window), None::<&gio::Cancellable>, move |result| {
            ctx.state.borrow_mut().conflict = None;
            if result == Ok(1) {
                ctx.reload(&watch, &disk_copy);
            } else {
                if let Some(entry) = ctx.state.borrow_mut().buffers.get_mut(&watch) {
                    entry.acked_external = Some(disk_copy);
                }
                // Правки, сделанные пока был открыт диалог, ещё не записаны.
                ctx.schedule_autosave(watch.clone());
            }
            let queued = std::mem::take(&mut ctx.state.borrow_mut().conflict_queue);
            for path in queued {
                ctx.on_disk_changed(&path);
            }
        });
    }

    pub(super) fn reload(&self, path: &Path, text: &str) {
        let Some(buffer) = self
            .state
            .borrow()
            .buffers
            .get(path)
            .map(|entry| entry.buffer.clone())
        else {
            return;
        };
        // Таблицы и картинки держатся на якорях в тексте; set_text их удаляет.
        self.drop_render_for_path(path);
        self.state.borrow_mut().loading = true;
        buffer.set_text(text);
        self.state.borrow_mut().loading = false;
        if let Some(entry) = self.state.borrow_mut().buffers.get_mut(path) {
            entry.dirty = false;
            entry.last_saved = text.to_string();
            entry.acked_external = None;
        }
        self.refresh_open_list();
        self.follow_focus();
        self.set_status("Файл перечитан с диска");
    }
}

fn same_file(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}
