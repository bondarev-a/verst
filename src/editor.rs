/// Как набирать текст для уже определённого языка.
/// Цвета задаёт тема окна, здесь только табуляция.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditProfile {
    pub tab_width: u32,
    pub insert_spaces: bool,
}

/// Профиль по идентификатору GtkSourceView. Неизвестный язык — 4 пробела.
pub fn profile_for(language_id: Option<&str>) -> EditProfile {
    match language_id {
        Some("go" | "makefile" | "meson") => EditProfile {
            tab_width: 8,
            insert_spaces: false,
        },
        _ => EditProfile {
            tab_width: 4,
            insert_spaces: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_for_go_make_and_meson() {
        for id in ["go", "makefile", "meson"] {
            let profile = profile_for(Some(id));
            assert!(!profile.insert_spaces, "{id}");
            assert_eq!(profile.tab_width, 8, "{id}");
        }
    }

    #[test]
    fn spaces_for_other_languages() {
        for id in [
            None,
            Some("rust"),
            Some("python3"),
            Some("markdown"),
            Some("toml"),
        ] {
            let profile = profile_for(id);
            assert!(profile.insert_spaces);
            assert_eq!(profile.tab_width, 4);
        }
    }
}
