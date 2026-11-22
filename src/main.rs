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
    // Профиль AppArmor закрывает только /usr/bin/verst. Локальная копия
    // иначе падает сразу: bwrap не может создать user namespace.
    if std::env::current_exe().ok().as_deref() != Some(std::path::Path::new("/usr/bin/verst")) {
        std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
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
