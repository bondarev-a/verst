// Copyright (C) 2026 alex
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

mod ui;

use gtk4::prelude::*;
use gtk4::gio;

fn main() -> gtk4::glib::ExitCode {
    // Профиль AppArmor закрывает только /usr/bin/verst. Локальная копия
    // иначе падает сразу: bwrap не может создать user namespace.
    if std::env::current_exe().ok().as_deref() != Some(std::path::Path::new("/usr/bin/verst")) {
        std::env::set_var("WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS", "1");
    }
    let app = gtk4::Application::builder()
        .application_id("local.textworkspace")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(ui::activate);
    app.connect_open(ui::open_files);
    app.run()
}
