// Copyright (C) 2026 alex
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! Песочница WebKit (bubblewrap).
//!
//! Предпросмотр показывает чужие файлы: HTML, SVG, картинки, видео.
//! Процесс страницы WebKit запускает внутри bwrap, и ошибка в движке или
//! декодере не даёт доступа к файлам пользователя. На Ubuntu 24.04 без
//! профиля AppArmor bwrap не может создать user namespace, и WebKit падает
//! при старте; тогда песочницу приходится выключать. Раньше её выключали для
//! любого бинарника не из /usr/bin/verst, хотя на многих системах (например,
//! Ubuntu 26.04 с профилем bwrap) она работает.

use std::process::{Command, Stdio};

const DISABLE: &str = "WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS";

/// Может ли bwrap создать песочницу так, как это делает WebKit.
pub fn bwrap_works() -> bool {
    Command::new("bwrap")
        .args([
            "--unshare-user",
            "--unshare-pid",
            "--unshare-net",
            "--die-with-parent",
            "--ro-bind",
            "/",
            "/",
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Выключить песочницу, только если она здесь не работает.
/// `trusted_path` — бинарник, для которого установлен профиль AppArmor.
/// Возвращает `true`, если песочница выключена.
pub fn disable_if_unusable(trusted_path: &std::path::Path) -> bool {
    if std::env::var_os(DISABLE).is_some() {
        return true;
    }
    if std::env::current_exe().ok().as_deref() == Some(trusted_path) {
        return false;
    }
    if bwrap_works() {
        return false;
    }
    std::env::set_var(DISABLE, "1");
    true
}

/// Песочница выключена (переменной окружения или проверкой при запуске).
pub fn disabled() -> bool {
    std::env::var_os(DISABLE).is_some()
}
