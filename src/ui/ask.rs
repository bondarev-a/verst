//! Чип у выделения. В запрос уходит этот фрагмент и вопрос, без файла и без пространства.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4::prelude::*;
use text_workspace::inference::{self, AskFail};
use text_workspace::session::ViewMode;

use super::Ctx;

#[derive(Clone)]
pub(super) struct Seen {
    buffer: sourceview5::Buffer,
    start: i32,
    end: i32,
    text: String,
}

pub(super) struct PendingAsk {
    question: String,
    selection: String,
}

#[derive(Clone)]
pub(super) struct AskHold {
    buffer: sourceview5::Buffer,
    start_mark: gtk4::TextMark,
    end_mark: gtk4::TextMark,
    text: String,
}

pub(super) struct AskUi {
    pub chip: gtk4::Button,
    pub popover: gtk4::Popover,
    pub quote: gtk4::Label,
    pub entry: gtk4::Entry,
    pub send: gtk4::Button,
    pub answer: gtk4::Label,
    pub answer_scroll: gtk4::ScrolledWindow,
    pub replace: gtk4::Button,
    pub insert: gtk4::Button,
    pub note: gtk4::Label,
    pub pending: Rc<RefCell<Option<PendingAsk>>>,
    pub hold: Rc<RefCell<Option<AskHold>>>,
    pub seen: Rc<RefCell<Option<Seen>>>,
    pub frozen: Rc<Cell<bool>>,
    pub hover: Rc<Cell<bool>>,
    pub waiting: Rc<Cell<bool>>,
    pub generation: Rc<Cell<u64>>,
}

pub(super) fn build_ask() -> AskUi {
    let chip = gtk4::Button::with_label("Отправить в AI");
    chip.add_css_class("ask-chip");
    chip.set_halign(gtk4::Align::Start);
    chip.set_valign(gtk4::Align::Start);
    chip.set_hexpand(false);
    chip.set_vexpand(false);
    chip.set_focus_on_click(false);
    chip.set_can_focus(false);
    chip.set_visible(false);
    chip.set_tooltip_text(Some("Отправить выделенный фрагмент в AI"));

    let quote = gtk4::Label::new(None);
    quote.add_css_class("dim-label");
    quote.set_halign(gtk4::Align::Start);
    quote.set_xalign(0.0);
    quote.set_wrap(false);
    quote.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    quote.set_max_width_chars(42);
    quote.set_selectable(true);

    let entry = gtk4::Entry::new();
    entry.set_placeholder_text(Some("Что сделать с этим фрагментом"));
    entry.set_hexpand(true);
    entry.set_width_chars(36);

    let send = gtk4::Button::with_label("Отправить");
    send.add_css_class("suggested-action");
    send.set_sensitive(false);

    let answer = gtk4::Label::new(None);
    answer.set_wrap(true);
    answer.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    answer.set_selectable(true);
    answer.set_xalign(0.0);
    answer.set_yalign(0.0);
    answer.set_halign(gtk4::Align::Start);
    answer.set_max_width_chars(42);
    let answer_scroll = gtk4::ScrolledWindow::new();
    answer_scroll.set_child(Some(&answer));
    answer_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    answer_scroll.set_propagate_natural_height(true);
    answer_scroll.set_max_content_height(220);
    answer_scroll.set_visible(false);

    let replace = gtk4::Button::with_label("Заменить выделение");
    replace.set_visible(false);
    let insert = gtk4::Button::with_label("Добавить ниже");
    insert.set_visible(false);
    let actions = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    actions.append(&replace);
    actions.append(&insert);

    let note = gtk4::Label::new(None);
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.set_halign(gtk4::Align::Start);
    note.set_max_width_chars(42);

    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.set_width_request(360);
    let title = gtk4::Label::new(Some("Отправить в AI"));
    title.set_halign(gtk4::Align::Start);
    title.add_css_class("heading");
    body.append(&title);
    body.append(&quote);
    body.append(&entry);
    body.append(&send);
    body.append(&note);
    body.append(&answer_scroll);
    body.append(&actions);

    let popover = gtk4::Popover::new();
    popover.add_css_class("ask-popover");
    popover.set_position(gtk4::PositionType::Bottom);
    popover.set_has_arrow(true);
    popover.set_child(Some(&body));
    popover.set_parent(&chip);

    AskUi {
        chip,
        popover,
        quote,
        entry,
        send,
        answer,
        answer_scroll,
        replace,
        insert,
        note,
        pending: Rc::new(RefCell::new(None)),
        hold: Rc::new(RefCell::new(None)),
        seen: Rc::new(RefCell::new(None)),
        frozen: Rc::new(Cell::new(false)),
        hover: Rc::new(Cell::new(false)),
        waiting: Rc::new(Cell::new(false)),
        generation: Rc::new(Cell::new(0)),
    }
}

impl Ctx {
    pub(super) fn install_ask_watch(&self) {
        for view in &self.widgets.views {
            if let Some(scroll) = super::scrolled_of(view) {
                let ctx = self.clone();
                scroll
                    .vadjustment()
                    .connect_value_changed(move |_| ctx.sync_ask_chip());
                let ctx = self.clone();
                scroll
                    .hadjustment()
                    .connect_value_changed(move |_| ctx.sync_ask_chip());
            }
            let ctx = self.clone();
            view.connect_notify_local(Some("buffer"), move |_, _| ctx.sync_ask_chip());
        }
        let motion = gtk4::EventControllerMotion::new();
        let ctx = self.clone();
        motion.connect_enter(move |_, _, _| {
            ctx.widgets.ask_hover.set(true);
        });
        let ctx = self.clone();
        motion.connect_leave(move |_| {
            ctx.widgets.ask_hover.set(false);
            ctx.sync_ask_chip();
        });
        self.widgets.ask_chip.add_controller(motion);

        let ctx = self.clone();
        self.widgets
            .ask_chip
            .connect_clicked(move |_| ctx.open_ask());
        let ctx = self.clone();
        self.widgets
            .ask_entry
            .connect_activate(move |_| ctx.send_ask());
        let ctx = self.clone();
        self.widgets
            .ask_entry
            .connect_changed(move |_| ctx.refresh_ask_send());
        let ctx = self.clone();
        self.widgets
            .ask_send
            .connect_clicked(move |_| ctx.send_ask());
        let ctx = self.clone();
        self.widgets
            .ask_replace
            .connect_clicked(move |_| ctx.replace_ask());
        let ctx = self.clone();
        self.widgets
            .ask_insert
            .connect_clicked(move |_| ctx.insert_ask());
        let ctx = self.clone();
        self.widgets
            .ask_popover
            .connect_closed(move |_| ctx.on_ask_closed());
        self.fill_ask_log();
    }

    pub(super) fn sync_ask_chip(&self) {
        if !self.editing_text() {
            self.widgets.ask_chip.set_visible(false);
            if self.widgets.ask_popover.is_visible() {
                self.widgets.ask_popover.popdown();
            } else {
                self.widgets.ask_frozen.set(false);
                self.release_hold();
            }
            return;
        }
        if self.widgets.ask_frozen.get() {
            self.widgets.ask_chip.set_visible(true);
            self.place_ask_chip();
            return;
        }
        if let Some(seen) = self.read_selection() {
            self.place_live(&seen);
            *self.widgets.ask_seen.borrow_mut() = Some(seen);
            self.widgets.ask_chip.set_visible(true);
            return;
        }
        if self.widgets.ask_hover.get() && self.widgets.ask_seen.borrow().is_some() {
            self.widgets.ask_chip.set_visible(true);
            return;
        }
        self.widgets.ask_chip.set_visible(false);
    }

    fn editing_text(&self) -> bool {
        let (browsing, mode, writing) = {
            let state = self.state.borrow();
            (state.browsing, state.view_mode, state.render.writing)
        };
        if browsing || writing || self.any_table_cell_focused() {
            return false;
        }
        if !matches!(mode, ViewMode::Edit | ViewMode::Render) {
            return false;
        }
        self.widgets.content_stack.visible_child_name().as_deref() == Some("edit")
    }

    fn read_selection(&self) -> Option<Seen> {
        let view = self.focused_view();
        let buffer = view.buffer().downcast::<sourceview5::Buffer>().ok()?;
        let (start, end) = buffer.selection_bounds()?;
        if start == end {
            return None;
        }
        let text = buffer.text(&start, &end, true).to_string();
        if text.trim().is_empty() {
            return None;
        }
        Some(Seen {
            buffer,
            start: start.offset(),
            end: end.offset(),
            text,
        })
    }

    fn place_live(&self, seen: &Seen) {
        let view = self.focused_view();
        let end = seen.buffer.iter_at_offset(seen.end);
        self.place_at(&view, &end);
    }

    fn place_ask_chip(&self) {
        let Some(hold) = self.widgets.ask_hold.borrow().clone() else {
            return;
        };
        let Some(view) = self.view_of(&hold.buffer) else {
            return;
        };
        let end = hold.buffer.iter_at_mark(&hold.end_mark);
        self.place_at(&view, &end);
    }

    fn view_of(&self, buffer: &sourceview5::Buffer) -> Option<sourceview5::View> {
        let wanted = buffer.clone().upcast::<gtk4::TextBuffer>();
        self.widgets
            .views
            .iter()
            .find(|view| view.buffer() == wanted)
            .cloned()
    }

    fn place_at(&self, view: &sourceview5::View, end: &gtk4::TextIter) {
        if !view.is_mapped() || view.width() < 40 || view.height() < 20 {
            return;
        }
        let rect = view.iter_location(end);
        let (wx, wy) = view.buffer_to_window_coords(
            gtk4::TextWindowType::Widget,
            rect.x(),
            rect.y() + rect.height() + 6,
        );
        let inside = wx >= 0 && wy >= 0 && wx <= view.width() && wy <= view.height() + 8;
        if !inside {
            if !self.widgets.ask_frozen.get() && !self.widgets.ask_hover.get() {
                self.widgets.ask_chip.set_visible(false);
            }
            return;
        }
        let point = gtk4::graphene::Point::new(wx as f32, wy as f32);
        let Some(at) = view.compute_point(&self.widgets.editor_overlay, &point) else {
            return;
        };
        let x = at.x();
        let y = at.y();
        let overlay_w = self.widgets.editor_overlay.width();
        let overlay_h = self.widgets.editor_overlay.height();
        let chip_w = self.widgets.ask_chip.width().max(200);
        let chip_h = self.widgets.ask_chip.height().max(28);
        let max_x = (overlay_w - chip_w - 8).max(8);
        let max_y = (overlay_h - chip_h - 8).max(8);
        self.widgets
            .ask_chip
            .set_margin_start((x as i32).clamp(8, max_x));
        self.widgets
            .ask_chip
            .set_margin_top((y as i32).clamp(8, max_y));
        self.widgets.ask_chip.set_margin_end(0);
        self.widgets.ask_chip.set_margin_bottom(0);
    }

    fn open_ask(&self) {
        self.widgets.ask_frozen.set(true);
        let Some(hold) = self.pin_selection() else {
            self.widgets.ask_frozen.set(false);
            self.show_ask_error(&AskFail::EmptySelection.to_string());
            self.widgets.ask_chip.set_visible(true);
            self.widgets.ask_popover.popup();
            return;
        };
        let quote = excerpt(&hold.text);
        self.remember_hold(hold);
        self.widgets.ask_quote.set_text(&quote);
        self.widgets.ask_answer.set_text("");
        self.widgets.ask_answer_scroll.set_visible(false);
        self.widgets.ask_replace.set_visible(false);
        self.widgets.ask_insert.set_visible(false);
        self.clear_ask_note();
        self.widgets.ask_chip.set_visible(true);
        self.place_ask_chip();
        self.refresh_ask_send();
        let ctx = self.clone();
        gtk4::glib::idle_add_local_once(move || {
            if !ctx.widgets.ask_frozen.get() {
                return;
            }
            ctx.widgets.ask_popover.popup();
            ctx.widgets.ask_entry.grab_focus_without_selecting();
        });
    }

    fn pin_selection(&self) -> Option<AskHold> {
        let seen = self
            .read_selection()
            .or_else(|| self.widgets.ask_seen.borrow().clone())?;
        let start = seen.buffer.iter_at_offset(seen.start);
        let end = seen.buffer.iter_at_offset(seen.end);
        if start == end {
            return None;
        }
        let text = seen.buffer.text(&start, &end, true).to_string();
        if text != seen.text {
            return None;
        }
        let start_mark = seen.buffer.create_mark(None, &start, true);
        let end_mark = seen.buffer.create_mark(None, &end, false);
        Some(AskHold {
            buffer: seen.buffer,
            start_mark,
            end_mark,
            text,
        })
    }

    fn remember_hold(&self, hold: AskHold) {
        let previous = self.widgets.ask_hold.borrow_mut().replace(hold);
        let Some(previous) = previous else {
            return;
        };
        let same = self
            .widgets
            .ask_hold
            .borrow()
            .as_ref()
            .is_some_and(|hold| hold.start_mark == previous.start_mark);
        if !same {
            previous.buffer.delete_mark(&previous.start_mark);
            previous.buffer.delete_mark(&previous.end_mark);
        }
    }

    fn release_hold(&self) {
        let previous = self.widgets.ask_hold.borrow_mut().take();
        let Some(previous) = previous else {
            return;
        };
        previous.buffer.delete_mark(&previous.start_mark);
        previous.buffer.delete_mark(&previous.end_mark);
    }

    fn on_ask_closed(&self) {
        self.widgets.ask_frozen.set(false);
        self.widgets
            .ask_gen
            .set(self.widgets.ask_gen.get().wrapping_add(1));
        self.set_ask_waiting(false);
        self.release_hold();
        self.sync_ask_chip();
    }

    fn send_ask(&self) {
        if self.widgets.ask_waiting.get() {
            return;
        }
        let question = self.widgets.ask_entry.text().trim().to_string();
        let Some(text) = self
            .widgets
            .ask_hold
            .borrow()
            .as_ref()
            .and_then(fragment_of)
        else {
            self.show_ask_error(&AskFail::EmptySelection.to_string());
            return;
        };
        if let Some(hold) = self.widgets.ask_hold.borrow_mut().as_mut() {
            hold.text = text.clone();
        }
        let access = inference::Access::load();
        let prepared = match inference::prepare(&access, &text, &question) {
            Ok(prepared) => prepared,
            Err(err) => {
                self.show_ask_error(&err.to_string());
                return;
            }
        };
        self.widgets
            .ask_gen
            .set(self.widgets.ask_gen.get().wrapping_add(1));
        let generation = self.widgets.ask_gen.get();
        self.set_ask_waiting(true);
        self.clear_ask_note();
        self.widgets.ask_note.set_text("Жду ответ…");
        self.widgets.ask_answer.set_text("");
        self.widgets.ask_answer_scroll.set_visible(false);
        self.widgets.ask_replace.set_visible(false);
        self.widgets.ask_insert.set_visible(false);
        *self.widgets.ask_pending.borrow_mut() = Some(PendingAsk {
            question: question.clone(),
            selection: text.clone(),
        });
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(inference::complete(&prepared));
        });
        let ctx = self.clone();
        gtk4::glib::timeout_add_local(Duration::from_millis(80), move || {
            match receiver.try_recv() {
                Ok(result) => {
                    ctx.finish_ask(generation, result);
                    gtk4::glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => gtk4::glib::ControlFlow::Break,
                Err(std::sync::mpsc::TryRecvError::Empty) => gtk4::glib::ControlFlow::Continue,
            }
        });
    }

    fn finish_ask(&self, generation: u64, result: Result<String, String>) {
        if self.widgets.ask_gen.get() != generation {
            return;
        }
        self.set_ask_waiting(false);
        self.clear_ask_note();
        match result {
            Ok(text) => {
                if let Some(pending) = self.widgets.ask_pending.borrow_mut().take() {
                    inference::remember_exchange(inference::Exchange {
                        question: pending.question,
                        selection: pending.selection,
                        answer: text.clone(),
                    });
                    self.fill_ask_log();
                }
                self.widgets.ask_answer.set_text(&text);
                self.widgets.ask_answer_scroll.set_visible(true);
                self.widgets.ask_replace.set_visible(true);
                self.widgets.ask_insert.set_visible(true);
            }
            Err(err) => self.show_ask_error(&err),
        }
    }

    fn replace_ask(&self) {
        let answer = self.widgets.ask_answer.text().to_string();
        if answer.trim().is_empty() {
            return;
        }
        let Some(hold) = self.widgets.ask_hold.borrow().clone() else {
            return;
        };
        if !self.buffer_is_open(&hold.buffer) {
            self.show_ask_error("Файл уже закрыт");
            return;
        }
        let Some(current) = fragment_of(&hold) else {
            self.show_ask_error("Выделение пропало");
            return;
        };
        if current != hold.text {
            self.show_ask_error("Фрагмент уже изменился");
            return;
        }
        let mut start = hold.buffer.iter_at_mark(&hold.start_mark);
        let mut end = hold.buffer.iter_at_mark(&hold.end_mark);
        hold.buffer.begin_user_action();
        hold.buffer.delete(&mut start, &mut end);
        hold.buffer.insert(&mut start, &answer);
        hold.buffer.end_user_action();
        self.set_status("Фрагмент заменён");
        self.widgets.ask_popover.popdown();
    }

    fn insert_ask(&self) {
        let answer = self.widgets.ask_answer.text().to_string();
        if answer.trim().is_empty() {
            return;
        }
        let Some(hold) = self.widgets.ask_hold.borrow().clone() else {
            return;
        };
        if !self.buffer_is_open(&hold.buffer) {
            self.show_ask_error("Файл уже закрыт");
            return;
        }
        let mut end = hold.buffer.iter_at_mark(&hold.end_mark);
        let current = fragment_of(&hold).unwrap_or_default();
        let prefix = if current.ends_with('\n') { "" } else { "\n" };
        hold.buffer.begin_user_action();
        hold.buffer
            .insert(&mut end, &format!("{prefix}{answer}"));
        hold.buffer.end_user_action();
        self.set_status("Ответ добавлен ниже");
        self.widgets.ask_popover.popdown();
    }

    fn buffer_is_open(&self, buffer: &sourceview5::Buffer) -> bool {
        self.state
            .borrow()
            .buffers
            .values()
            .any(|entry| entry.buffer == *buffer)
    }

    fn show_ask_error(&self, text: &str) {
        self.widgets.ask_note.add_css_class("ask-error");
        self.widgets.ask_note.set_text(text);
    }

    fn clear_ask_note(&self) {
        self.widgets.ask_note.remove_css_class("ask-error");
        self.widgets.ask_note.set_text("");
    }

    fn refresh_ask_send(&self) {
        let ready =
            !self.widgets.ask_waiting.get() && !self.widgets.ask_entry.text().trim().is_empty();
        self.widgets.ask_send.set_sensitive(ready);
    }

    fn set_ask_waiting(&self, waiting: bool) {
        self.widgets.ask_waiting.set(waiting);
        self.widgets.ask_entry.set_sensitive(!waiting);
        self.widgets.ask_replace.set_sensitive(!waiting);
        self.widgets.ask_insert.set_sensitive(!waiting);
        self.refresh_ask_send();
    }

    fn fill_ask_log(&self) {
        let log = &self.widgets.ask_log;
        while let Some(child) = log.first_child() {
            log.remove(&child);
        }
        let items = inference::load_exchanges();
        if items.is_empty() {
            log.append(&dim_side("Запросов ещё нет"));
            return;
        }
        for item in items {
            log.append(&exchange_row(&item));
        }
    }
}

fn fragment_of(hold: &AskHold) -> Option<String> {
    let start = hold.buffer.iter_at_mark(&hold.start_mark);
    let end = hold.buffer.iter_at_mark(&hold.end_mark);
    if start == end {
        return None;
    }
    let text = hold.buffer.text(&start, &end, true).to_string();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn exchange_row(item: &inference::Exchange) -> gtk4::Expander {
    let question = gtk4::Label::new(Some(&item.question));
    question.set_halign(gtk4::Align::Start);
    question.set_hexpand(true);
    question.set_xalign(0.0);
    question.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    question.set_max_width_chars(28);
    let preview = gtk4::Label::new(Some(&clip(&item.answer, 80)));
    preview.add_css_class("dim-label");
    preview.set_halign(gtk4::Align::Start);
    preview.set_hexpand(true);
    preview.set_xalign(0.0);
    preview.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    preview.set_max_width_chars(28);
    let head = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    head.append(&question);
    head.append(&preview);

    let sent = side_text(&format!("Отправлено\n{}", item.selection));
    let got = side_text(&format!("Получено\n{}", item.answer));
    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 6);
    body.set_margin_top(4);
    body.set_margin_bottom(6);
    body.append(&sent);
    body.append(&got);

    let row = gtk4::Expander::new(None);
    row.set_label_widget(Some(&head));
    row.set_child(Some(&body));
    row.set_margin_start(8);
    row.set_margin_end(8);
    row.set_margin_top(2);
    let preview_hide = preview.clone();
    row.connect_expanded_notify(move |row| {
        preview_hide.set_visible(!row.is_expanded());
    });
    row
}

fn side_text(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.set_wrap(true);
    label.set_wrap_mode(gtk4::pango::WrapMode::WordChar);
    label.set_xalign(0.0);
    label.set_halign(gtk4::Align::Start);
    label.set_selectable(true);
    label.set_max_width_chars(28);
    label
}

fn dim_side(text: &str) -> gtk4::Label {
    let label = gtk4::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_halign(gtk4::Align::Start);
    label.set_margin_start(12);
    label.set_margin_top(4);
    label.set_margin_bottom(4);
    label
}

fn clip(text: &str, limit: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = flat.len();
    let mut count = 0;
    for (index, _) in flat.char_indices() {
        if count == limit {
            end = index;
            break;
        }
        count += 1;
    }
    if end < flat.len() {
        format!("{}…", &flat[..end])
    } else {
        flat
    }
}

fn excerpt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = flat.len();
    let mut count = 0;
    for (index, _) in flat.char_indices() {
        if count == 80 {
            end = index;
            break;
        }
        count += 1;
    }
    if end < flat.len() {
        format!("{}…", &flat[..end])
    } else {
        flat
    }
}
