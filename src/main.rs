// Copyright (C) 2026 alex
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

mod ui;

use gtk4::gio;
use gtk4::prelude::*;

fn main() -> gtk4::glib::ExitCode {
    ui::prof_mark_main();
    // Без аппаратного 3D GTK рисует через программный Vulkan, и запуск
    // почти вдвое дольше. Тогда cairo, если пользователь не выбрал сам.
    text_workspace::graphics::prefer_cairo_without_gpu();
    text_workspace::graphics::lighten_webkit_without_gpu();
    // Профиль AppArmor пакета открывает user namespace для /usr/bin/verst.
    // Другой копии песочница WebKit нужна так же; выключаем её, только если
    // bwrap на этой системе её создать не может (иначе WebKit падает).
    if text_workspace::sandbox::disable_if_unusable(std::path::Path::new("/usr/bin/verst")) {
        eprintln!("verst: песочница WebKit выключена — bwrap не может её создать");
    }
    let app = gtk4::Application::builder()
        .application_id("local.textworkspace")
        .flags(if std::env::var_os("VERST_PROF").is_some() {
            gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE
        } else {
            gio::ApplicationFlags::HANDLES_OPEN
        })
        .build();
    app.connect_activate(ui::activate);
    app.connect_open(ui::open_files);
    app.run()
}
