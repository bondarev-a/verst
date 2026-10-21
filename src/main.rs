mod ui;

use gtk4::prelude::*;
use gtk4::gio;

fn main() -> gtk4::glib::ExitCode {
    let app = gtk4::Application::builder()
        .application_id("local.textworkspace")
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(ui::activate);
    app.connect_open(ui::open_files);
    app.run()
}
