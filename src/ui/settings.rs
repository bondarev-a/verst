use std::path::Path;
use std::rc::Rc;

use gtk4::prelude::*;
use sourceview5::prelude::{BufferExt, ViewExt};
use text_workspace::session::{self, ViewMode};
use text_workspace::zoom;

use super::Ctx;

const STYLE_SAMPLE: &str = "\
/// Пример стиля
fn sample(name: &str) -> i32 {
    let text = \"строка\";
    // комментарий
    40 + name.len() as i32
}
";

impl Ctx {
    pub(super) fn show_preferences(&self) {
        if let Some(window) = self.widgets.preferences.borrow().clone() {
            window.present();
            return;
        }
        let session = self.state.borrow().session.clone();
        let window = gtk4::Window::builder()
            .transient_for(&self.widgets.window)
            .destroy_with_parent(true)
            .title("Настройки")
            .default_width(760)
            .build();
        let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
        box_.set_hexpand(true);
        let columns = gtk4::Box::new(gtk4::Orientation::Horizontal, 16);
        columns.set_margin_top(16);
        columns.set_margin_bottom(16);
        columns.set_margin_start(16);
        columns.set_margin_end(16);

        let open_spin = gtk4::SpinButton::with_range(
            f64::from(session::OPEN_LIMIT_MIN),
            f64::from(session::OPEN_LIMIT_MAX),
            1.0,
        );
        open_spin.set_digits(0);
        open_spin.set_numeric(true);
        open_spin.set_width_chars(4);
        open_spin.set_value(f64::from(session.open_limit));
        open_spin.set_tooltip_text(Some(
            "Сколько файлов держать в списке «Открытые». Лишние закрываются, начиная с давно не открывавшихся. Файл, который сейчас на экране, остаётся.",
        ));
        let ctx = self.clone();
        open_spin.connect_value_changed(move |spin| {
            ctx.set_open_limit(spin.value() as u32);
        });
        box_.append(&labeled("Открытых файлов", &open_spin));

        let themes = theme_names();
        let theme_refs: Vec<&str> = themes.iter().map(String::as_str).collect();
        let theme_dropdown = gtk4::DropDown::from_strings(&theme_refs);
        let theme_index = themes
            .iter()
            .position(|name| name == &session.theme)
            .unwrap_or(0) as u32;
        theme_dropdown.set_selected(theme_index);
        theme_dropdown.set_tooltip_text(Some(
            "Цвета подсветки в режиме редактирования. На просмотр не влияет.",
        ));
        let sample = style_sample();
        paint_sample(&sample, &session.theme);
        let ctx = self.clone();
        let theme_names = Rc::new(themes);
        let sample_buffer = sample.clone();
        theme_dropdown.connect_selected_notify(move |dropdown| {
            let Some(theme) = theme_names.get(dropdown.selected() as usize) else {
                return;
            };
            paint_sample(&sample_buffer, theme);
            ctx.set_theme(theme);
        });
        box_.append(&labeled("Тема исходника", &theme_dropdown));

        let open_mode = gtk4::DropDown::from_strings(&["Редактирование", "Просмотр"]);
        open_mode.set_selected(if session.open_mode == ViewMode::Preview {
            1
        } else {
            0
        });
        open_mode.set_tooltip_text(Some(
            "Как открывать следующий файл. Кнопка в углу переключает вид текущего.",
        ));
        let ctx = self.clone();
        open_mode.connect_selected_notify(move |dropdown| {
            let mode = if dropdown.selected() == 1 {
                ViewMode::Preview
            } else {
                ViewMode::Edit
            };
            ctx.set_open_mode(mode);
        });
        *self.widgets.open_mode_settings.borrow_mut() = Some(open_mode.clone());
        box_.append(&labeled("Открывать", &open_mode));

        let wrap = gtk4::Switch::new();
        wrap.set_active(session.word_wrap);
        wrap.set_valign(gtk4::Align::Center);
        wrap.set_tooltip_text(Some(
            "Переносить длинные строки по ширине редактора. Очень длинное слово тоже уходит на следующую строку.",
        ));
        let ctx = self.clone();
        wrap.connect_active_notify(move |switch| {
            ctx.set_word_wrap(switch.is_active());
        });
        *self.widgets.word_wrap_switch.borrow_mut() = Some(wrap.clone());
        box_.append(&labeled("Перенос строк", &wrap));

        let zoom_spin =
            gtk4::SpinButton::with_range(f64::from(zoom::MIN_PT), f64::from(zoom::MAX_PT), 1.0);
        zoom_spin.set_digits(0);
        zoom_spin.set_numeric(true);
        zoom_spin.set_width_chars(4);
        zoom_spin.set_value(f64::from(session.zoom_pt));
        zoom_spin.set_tooltip_text(Some("Размер текста. Те же клавиши Ctrl++ и Ctrl+−."));
        let ctx = self.clone();
        zoom_spin.connect_value_changed(move |spin| {
            ctx.set_zoom_pt(spin.value() as i32);
        });
        *self.widgets.zoom_spin.borrow_mut() = Some(zoom_spin.clone());
        box_.append(&labeled("Масштаб, пт", &zoom_spin));

        let sample_view = sourceview5::View::with_buffer(&sample);
        sample_view.set_editable(false);
        sample_view.set_cursor_visible(false);
        sample_view.set_monospace(true);
        sample_view.set_show_line_numbers(true);
        sample_view.set_left_margin(8);
        sample_view.set_right_margin(8);
        let sample_scroll = gtk4::ScrolledWindow::new();
        sample_scroll.set_child(Some(&sample_view));
        sample_scroll.set_size_request(300, 180);
        sample_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Never);
        let sample_frame = gtk4::Frame::new(Some("Пример стиля"));
        sample_frame.set_child(Some(&sample_scroll));
        sample_frame.set_valign(gtk4::Align::Start);
        columns.append(&box_);
        columns.append(&sample_frame);
        window.set_child(Some(&columns));
        let preferences = self.widgets.preferences.clone();
        let zoom_slot = self.widgets.zoom_spin.clone();
        let open_mode_slot = self.widgets.open_mode_settings.clone();
        let wrap_slot = self.widgets.word_wrap_switch.clone();
        window.connect_close_request(move |_| {
            preferences.borrow_mut().take();
            zoom_slot.borrow_mut().take();
            open_mode_slot.borrow_mut().take();
            wrap_slot.borrow_mut().take();
            gtk4::glib::Propagation::Proceed
        });
        *self.widgets.preferences.borrow_mut() = Some(window.clone());
        window.present();
    }

    pub(super) fn set_open_limit(&self, limit: u32) {
        let limit = limit.clamp(session::OPEN_LIMIT_MIN, session::OPEN_LIMIT_MAX);
        if self.state.borrow().session.open_limit == limit {
            return;
        }
        self.state.borrow_mut().session.open_limit = limit;
        self.enforce_open_limit();
        self.save_session();
    }

    pub(super) fn set_open_mode(&self, mode: ViewMode) {
        if self.state.borrow().loading {
            return;
        }
        if self.state.borrow().session.open_mode == mode {
            self.sync_open_dropdowns(mode);
            return;
        }
        self.state.borrow_mut().session.open_mode = mode;
        self.sync_open_dropdowns(mode);
        self.save_session();
    }

    fn sync_open_dropdowns(&self, mode: ViewMode) {
        let index = if mode == ViewMode::Preview { 1 } else { 0 };
        if self.widgets.open_mode_dropdown.selected() != index {
            self.widgets.open_mode_dropdown.set_selected(index);
        }
        if let Some(dropdown) = self.widgets.open_mode_settings.borrow().clone() {
            if dropdown.selected() != index {
                dropdown.set_selected(index);
            }
        }
    }

    pub(super) fn set_zoom_pt(&self, pt: i32) {
        let pt = zoom::clamp_pt(pt);
        if self.state.borrow().session.zoom_pt == pt {
            return;
        }
        self.state.borrow_mut().session.zoom_pt = pt;
        self.apply_zoom(pt);
        self.save_session();
    }

    pub(super) fn set_word_wrap(&self, wrap: bool) {
        if self.state.borrow().session.word_wrap == wrap {
            return;
        }
        self.state.borrow_mut().session.word_wrap = wrap;
        self.apply_wrap();
        self.save_session();
        if let Some(switch) = self.widgets.word_wrap_switch.borrow().clone() {
            if switch.is_active() != wrap {
                switch.set_active(wrap);
            }
        }
        if let Some(action) = self
            .widgets
            .window
            .application()
            .and_then(|app| app.lookup_action("word_wrap"))
            .and_then(|action| action.downcast::<gtk4::gio::SimpleAction>().ok())
        {
            let current = action
                .state()
                .and_then(|state| state.get::<bool>())
                .unwrap_or(!wrap);
            if current != wrap {
                action.set_state(&wrap.to_variant());
            }
        }
        self.set_status(if wrap {
            "Перенос строк включён"
        } else {
            "Перенос строк выключен"
        });
    }
}

fn style_sample() -> sourceview5::Buffer {
    let buffer = sourceview5::Buffer::new(None);
    buffer.set_highlight_syntax(true);
    buffer.set_text(STYLE_SAMPLE);
    let language =
        sourceview5::LanguageManager::default().guess_language(Some(Path::new("sample.rs")), None);
    buffer.set_language(language.as_ref());
    buffer
}

fn paint_sample(buffer: &sourceview5::Buffer, theme: &str) {
    let scheme = sourceview5::StyleSchemeManager::default().scheme(theme);
    buffer.set_style_scheme(scheme.as_ref());
}

fn theme_names() -> Vec<String> {
    let ids = sourceview5::StyleSchemeManager::default().scheme_ids();
    let names: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
    if names.is_empty() {
        vec!["Adwaita".to_string()]
    } else {
        names
    }
}

fn labeled(title: &str, control: &impl gtk4::prelude::IsA<gtk4::Widget>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
    let label = gtk4::Label::new(Some(title));
    label.set_halign(gtk4::Align::Start);
    label.set_hexpand(true);
    control.set_halign(gtk4::Align::End);
    row.append(&label);
    row.append(control);
    row
}
