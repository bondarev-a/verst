use std::path::Path;
use std::rc::Rc;

use gtk4::prelude::*;
use sourceview5::prelude::{BufferExt, ViewExt};
use text_workspace::inference::{Access, AccessDraft, KeyPlace, Provider};
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
        window.add_css_class("verst");
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
        // Лимит применяется после паузы: раньше каждый шаг стрелки или
        // промежуточное значение при наборе («5» на пути к «50») сразу
        // закрывали лишние файлы, и вернуть их было нельзя.
        let ctx = self.clone();
        let pending: std::rc::Rc<std::cell::Cell<u64>> = Default::default();
        open_spin.connect_value_changed(move |spin| {
            let tick = pending.get().wrapping_add(1);
            pending.set(tick);
            let value = spin.value() as u32;
            let ctx = ctx.clone();
            let pending = pending.clone();
            gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
                if pending.get() == tick {
                    ctx.set_open_limit(value);
                }
            });
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
        open_mode.set_selected(super::mode_dropdown_index(session.open_mode));
        open_mode.set_tooltip_text(Some(
            "Как открывать следующий файл. Кнопка в углу переключает вид текущего.",
        ));
        let ctx = self.clone();
        open_mode.connect_selected_notify(move |dropdown| {
            ctx.set_open_mode(super::mode_from_dropdown(dropdown.selected()));
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
        zoom_spin.set_tooltip_text(Some(
            "Интерфейс: дерево, меню, кнопки. Клавиши Ctrl++ и Ctrl+−.",
        ));
        let ctx = self.clone();
        zoom_spin.connect_value_changed(move |spin| {
            ctx.set_zoom_pt(spin.value() as i32);
        });
        *self.widgets.zoom_spin.borrow_mut() = Some(zoom_spin.clone());
        box_.append(&labeled("Масштаб окна, пт", &zoom_spin));

        let text_spin =
            gtk4::SpinButton::with_range(f64::from(zoom::MIN_PT), f64::from(zoom::MAX_PT), 1.0);
        text_spin.set_digits(0);
        text_spin.set_numeric(true);
        text_spin.set_width_chars(4);
        text_spin.set_value(f64::from(session.text_pt));
        text_spin.set_tooltip_text(Some(
            "Редактор и предпросмотр. Клавиши Ctrl+Shift++ и Ctrl+Shift+−.",
        ));
        let ctx = self.clone();
        text_spin.connect_value_changed(move |spin| {
            ctx.set_text_pt(spin.value() as i32);
        });
        *self.widgets.text_spin.borrow_mut() = Some(text_spin.clone());
        box_.append(&labeled("Масштаб текста, пт", &text_spin));

        let sample_view = sourceview5::View::with_buffer(&sample);
        sample_view.set_editable(false);
        sample_view.set_cursor_visible(false);
        sample_view.set_monospace(true);
        sample_view.add_css_class("tw-editor");
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
        let page = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
        page.append(&columns);
        mount_inference(&page);
        let scroll = gtk4::ScrolledWindow::new();
        scroll.set_child(Some(&page));
        scroll.set_propagate_natural_width(true);
        scroll.set_min_content_height(420);
        window.set_child(Some(&scroll));
        window.set_default_height(680);
        let preferences = self.widgets.preferences.clone();
        let zoom_slot = self.widgets.zoom_spin.clone();
        let text_slot = self.widgets.text_spin.clone();
        let open_mode_slot = self.widgets.open_mode_settings.clone();
        let wrap_slot = self.widgets.word_wrap_switch.clone();
        window.connect_close_request(move |_| {
            preferences.borrow_mut().take();
            zoom_slot.borrow_mut().take();
            text_slot.borrow_mut().take();
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
        let index = super::mode_dropdown_index(mode);
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
        self.apply_ui_zoom();
        self.save_session();
    }

    pub(super) fn set_text_pt(&self, pt: i32) {
        let pt = zoom::clamp_pt(pt);
        if self.state.borrow().session.text_pt == pt {
            return;
        }
        self.state.borrow_mut().session.text_pt = pt;
        self.apply_text_zoom();
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

fn mount_inference(page: &gtk4::Box) {
    let access = Access::load();
    let frame = gtk4::Frame::new(Some("Инференс"));
    frame.set_margin_start(16);
    frame.set_margin_end(16);
    frame.set_margin_bottom(16);
    let box_ = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    box_.set_margin_top(12);
    box_.set_margin_bottom(12);
    box_.set_margin_start(12);
    box_.set_margin_end(12);

    let provider = gtk4::DropDown::from_strings(&["OpenAI-совместимый", "Anthropic", "Выключен"]);
    provider.set_selected(provider_index(access.provider));
    provider.set_tooltip_text(Some(
        "OpenAI-совместимый адрес получает /chat/completions. Anthropic — /v1/messages. Выключен не отправляет выделение.",
    ));

    let base_url = gtk4::Entry::new();
    base_url.set_text(&access.base_url);
    base_url.set_hexpand(true);
    base_url.set_placeholder_text(Some("https://api.openai.com/v1"));
    base_url.set_tooltip_text(Some(
        "Базовый адрес, как base_url в AIGent. Без завершающего слэша тоже подходит.",
    ));

    let model = gtk4::Entry::new();
    model.set_text(&access.model);
    model.set_hexpand(true);
    model.set_placeholder_text(Some("имя модели"));
    model.set_tooltip_text(Some("Имя модели на этом адресе."));

    let api_key = gtk4::PasswordEntry::new();
    api_key.set_show_peek_icon(true);
    api_key.set_hexpand(true);
    let key_placeholder = if access.api_key.is_empty() {
        "ключ"
    } else {
        "ключ записан, пустое поле его не меняет"
    };
    api_key.set_placeholder_text(Some(key_placeholder));
    api_key.set_tooltip_text(Some(
        "Пустое поле не меняет уже записанный ключ. Если ключ в файле пуст, читается переменная окружения.",
    ));

    let api_key_env = gtk4::Entry::new();
    api_key_env.set_text(&access.api_key_env);
    api_key_env.set_hexpand(true);
    api_key_env.set_tooltip_text(Some(
        "Имя переменной, если ключ в файле пустой. По умолчанию LLM_API_KEY.",
    ));

    let key_line = gtk4::Label::new(None);
    key_line.set_halign(gtk4::Align::Start);
    key_line.set_xalign(0.0);
    key_line.add_css_class("dim-label");
    key_line.set_wrap(true);
    paint_key_line(&key_line, &access);

    let hint = gtk4::Label::new(Some(
        "В запрос уходит только выделенный фрагмент. Записанный ключ важнее переменной окружения.",
    ));
    hint.set_halign(gtk4::Align::Start);
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("dim-label");

    let status = gtk4::Label::new(None);
    status.set_halign(gtk4::Align::Start);
    status.set_xalign(0.0);

    let save = gtk4::Button::with_label("Сохранить доступ");
    save.add_css_class("suggested-action");
    let clear = gtk4::Button::with_label("Стереть ключ");
    clear.set_tooltip_text(Some("Стирает записанный ключ и сохраняет остальные поля."));

    let provider_save = provider.clone();
    let base_url_save = base_url.clone();
    let model_save = model.clone();
    let api_key_save = api_key.clone();
    let env_save = api_key_env.clone();
    let key_line_save = key_line.clone();
    let status_save = status.clone();
    save.connect_clicked(move |_| {
        write_inference(
            &provider_save,
            &base_url_save,
            &model_save,
            &api_key_save,
            &env_save,
            false,
            &key_line_save,
            &status_save,
        );
    });
    let provider_clear = provider.clone();
    let base_url_clear = base_url.clone();
    let model_clear = model.clone();
    let api_key_clear = api_key.clone();
    let env_clear = api_key_env.clone();
    let key_line_clear = key_line.clone();
    let status_clear = status.clone();
    clear.connect_clicked(move |_| {
        api_key_clear.set_text("");
        write_inference(
            &provider_clear,
            &base_url_clear,
            &model_clear,
            &api_key_clear,
            &env_clear,
            true,
            &key_line_clear,
            &status_clear,
        );
    });

    let buttons = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    buttons.append(&save);
    buttons.append(&clear);

    box_.append(&field("Провайдер", &provider));
    box_.append(&field("Адрес", &base_url));
    box_.append(&field("Модель", &model));
    box_.append(&field("Ключ", &api_key));
    box_.append(&field("Переменная", &api_key_env));
    box_.append(&key_line);
    box_.append(&hint);
    box_.append(&buttons);
    box_.append(&status);
    frame.set_child(Some(&box_));
    page.append(&frame);
}

fn write_inference(
    provider: &gtk4::DropDown,
    base_url: &gtk4::Entry,
    model: &gtk4::Entry,
    api_key: &gtk4::PasswordEntry,
    api_key_env: &gtk4::Entry,
    clear_key: bool,
    key_line: &gtk4::Label,
    status: &gtk4::Label,
) {
    let mut access = Access::load();
    access.apply_draft(AccessDraft {
        provider: provider_from_index(provider.selected()),
        base_url: base_url.text().to_string(),
        api_key: api_key.text().to_string(),
        clear_key,
        api_key_env: api_key_env.text().to_string(),
        model: model.text().to_string(),
    });
    match access.save() {
        Ok(()) => {
            paint_key_line(key_line, &access);
            if access.api_key.is_empty() {
                api_key.set_placeholder_text(Some("ключ"));
            } else {
                api_key.set_text("");
                api_key.set_placeholder_text(Some("ключ записан, пустое поле его не меняет"));
            }
            status.remove_css_class("ask-error");
            status.set_text(if clear_key {
                "Ключ стёрт"
            } else {
                "Доступ сохранён"
            });
        }
        Err(err) => {
            status.add_css_class("ask-error");
            status.set_text(&format!("Не удалось записать настройки: {err}"));
        }
    }
}

fn paint_key_line(label: &gtk4::Label, access: &Access) {
    let text = match access.key_place() {
        KeyPlace::File => "Ключ записан в файле настроек.".to_string(),
        KeyPlace::Env => format!(
            "Сохранённого ключа нет, при запросе берётся {}.",
            access.api_key_env
        ),
        KeyPlace::Missing => "Ключ не задан.".to_string(),
    };
    label.set_text(&text);
}

fn provider_index(provider: Provider) -> u32 {
    match provider {
        Provider::OpenaiCompatible => 0,
        Provider::Anthropic => 1,
        Provider::None => 2,
    }
}

fn provider_from_index(index: u32) -> Provider {
    match index {
        1 => Provider::Anthropic,
        2 => Provider::None,
        _ => Provider::OpenaiCompatible,
    }
}

fn field(title: &str, control: &impl gtk4::prelude::IsA<gtk4::Widget>) -> gtk4::Box {
    let row = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    let label = gtk4::Label::new(Some(title));
    label.set_halign(gtk4::Align::Start);
    label.add_css_class("dim-label");
    control.set_halign(gtk4::Align::Fill);
    row.append(&label);
    row.append(control);
    row
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
