pub struct CommandSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub section: &'static str,
    pub accels: &'static [&'static str],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelSide {
    Left,
    Right,
}

pub struct PanelSpec {
    pub id: &'static str,
    pub title: &'static str,
    pub side: PanelSide,
}

const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "save",
        title: "Сохранить",
        section: "Файл",
        accels: &["<Control>s"],
    },
    CommandSpec {
        id: "quick_note",
        title: "Быстрая заметка",
        section: "Файл",
        accels: &["<Control>n"],
    },
    CommandSpec {
        id: "pin",
        title: "Запинить каталог",
        section: "Файл",
        accels: &[],
    },
    CommandSpec {
        id: "pin_file",
        title: "Закрепить файл",
        section: "Файл",
        accels: &[],
    },
    CommandSpec {
        id: "close_file",
        title: "Закрыть",
        section: "Файл",
        accels: &["<Control>w"],
    },
    CommandSpec {
        id: "quick_open",
        title: "Перейти к файлу",
        section: "Файл",
        accels: &["<Control>p"],
    },
    CommandSpec {
        id: "preferences",
        title: "Настройки",
        section: "Файл",
        accels: &[],
    },
    CommandSpec {
        id: "quit",
        title: "Выход",
        section: "Файл",
        accels: &[],
    },
    CommandSpec {
        id: "undo",
        title: "Отменить",
        section: "Правка",
        accels: &["<Control>z"],
    },
    CommandSpec {
        id: "redo",
        title: "Повторить",
        section: "Правка",
        accels: &["<Control><Shift>z", "<Control>y"],
    },
    CommandSpec {
        id: "cut",
        title: "Вырезать",
        section: "Правка",
        accels: &["<Control>x"],
    },
    CommandSpec {
        id: "copy",
        title: "Копировать",
        section: "Правка",
        accels: &["<Control>c"],
    },
    CommandSpec {
        id: "paste",
        title: "Вставить",
        section: "Правка",
        accels: &["<Control>v"],
    },
    CommandSpec {
        id: "select_all",
        title: "Выделить всё",
        section: "Правка",
        accels: &["<Control>a"],
    },
    CommandSpec {
        id: "back",
        title: "Назад",
        section: "Вид",
        accels: &["<Alt>Left"],
    },
    CommandSpec {
        id: "forward",
        title: "Вперёд",
        section: "Вид",
        accels: &["<Alt>Right"],
    },
    CommandSpec {
        id: "show_history",
        title: "История файлов",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "split",
        title: "Разделить панель",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "word_wrap",
        title: "Перенос строк",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "zoom_in",
        title: "Увеличить",
        section: "Вид",
        accels: &["<Control>plus", "<Control>equal", "<Control>KP_Add"],
    },
    CommandSpec {
        id: "zoom_out",
        title: "Уменьшить",
        section: "Вид",
        accels: &["<Control>minus", "<Control>KP_Subtract"],
    },
    CommandSpec {
        id: "search",
        title: "Поиск по содержимому",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "find",
        title: "Найти в файле",
        section: "Вид",
        accels: &["<Control>f"],
    },
    CommandSpec {
        id: "reveal_disk",
        title: "Каталог в проводнике",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "reveal_workspace",
        title: "Каталог в пространстве",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "browse_folder",
        title: "Содержимое каталога",
        section: "Вид",
        accels: &[],
    },
    CommandSpec {
        id: "next_space",
        title: "Следующее пространство",
        section: "Вид",
        accels: &["<Control>Tab", "<Control><Alt>Right"],
    },
    CommandSpec {
        id: "previous_space",
        title: "Предыдущее пространство",
        section: "Вид",
        accels: &[
            "<Control><Shift>Tab",
            "<Control><Shift>ISO_Left_Tab",
            "<Control><Alt>Left",
        ],
    },
];

const PANELS: &[PanelSpec] = &[
    PanelSpec {
        id: "disk",
        title: "Проводник",
        side: PanelSide::Left,
    },
    PanelSpec {
        id: "workspace",
        title: "Пространства",
        side: PanelSide::Left,
    },
    PanelSpec {
        id: "notes",
        title: "Заметки",
        side: PanelSide::Left,
    },
    PanelSpec {
        id: "open",
        title: "Открытые",
        side: PanelSide::Left,
    },
    PanelSpec {
        id: "history",
        title: "История",
        side: PanelSide::Left,
    },
    PanelSpec {
        id: "outline",
        title: "Оглавление",
        side: PanelSide::Right,
    },
    PanelSpec {
        id: "search",
        title: "Поиск",
        side: PanelSide::Right,
    },
    PanelSpec {
        id: "backlinks",
        title: "Ссылки",
        side: PanelSide::Right,
    },
    PanelSpec {
        id: "graph",
        title: "Граф",
        side: PanelSide::Right,
    },
];

pub fn commands() -> &'static [CommandSpec] {
    COMMANDS
}

pub fn panels() -> &'static [PanelSpec] {
    PANELS
}

pub fn panel_icon(id: &str) -> &'static str {
    match id {
        "workspace" => "folder-visiting-symbolic",
        "disk" => "system-file-manager-symbolic",
        "notes" => "document-edit-symbolic",
        "open" => "document-open-symbolic",
        "history" => "document-open-recent-symbolic",
        _ => "image-missing-symbolic",
    }
}

pub fn panel_hint(id: &str) -> &'static str {
    match id {
        "disk" => "Проводник. Каталоги и файлы на диске",
        "workspace" => "Пространства. Запиненные каталоги",
        "notes" => "Заметки. Быстрые заметки вне пространства",
        "open" => "Открытые. Файлы, которые открыты сейчас, и закреплённые",
        "history" => "История. Недавно открытые файлы",
        _ => "",
    }
}

pub fn panel_id(side: PanelSide, index: u32) -> Option<&'static str> {
    panels()
        .iter()
        .filter(|panel| panel.side == side)
        .nth(index as usize)
        .map(|panel| panel.id)
}

pub fn ordered_ids(side: PanelSide, saved: &[String]) -> Vec<String> {
    let known: Vec<&str> = panels()
        .iter()
        .filter(|panel| panel.side == side)
        .map(|panel| panel.id)
        .collect();
    let mut order = Vec::new();
    for id in saved {
        if known.contains(&id.as_str()) && !order.iter().any(|item: &String| item == id) {
            order.push(id.clone());
        }
    }
    for id in known {
        if !order.iter().any(|item| item == id) {
            order.push(id.to_string());
        }
    }
    order
}

pub fn panel_index(id: &str) -> Option<u32> {
    let panel = panels().iter().find(|panel| panel.id == id)?;
    panels()
        .iter()
        .filter(|item| item.side == panel.side)
        .position(|item| item.id == id)
        .map(|index| index as u32)
}

pub fn canonical_panel(side: PanelSide, id: &str, legacy_page: u32) -> String {
    if panels()
        .iter()
        .any(|panel| panel.id == id && panel.side == side)
    {
        return id.to_string();
    }
    panel_id(side, legacy_page)
        .or_else(|| panel_id(side, 0))
        .unwrap_or(if side == PanelSide::Left {
            "disk"
        } else {
            "outline"
        })
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_and_save_have_accelerators() {
        let save = commands().iter().find(|c| c.id == "save").unwrap();
        assert!(save.accels.iter().any(|a| a.contains("<Control>s")));
        let zoom_in = commands().iter().find(|c| c.id == "zoom_in").unwrap();
        assert!(zoom_in.accels.iter().any(|a| a.contains("plus")));
        let zoom_out = commands().iter().find(|c| c.id == "zoom_out").unwrap();
        assert!(zoom_out.accels.iter().any(|a| a.contains("minus")));
        assert_eq!(panel_index("notes"), Some(2));
        assert_eq!(panel_index("open"), Some(3));
        assert_eq!(panel_index("history"), Some(4));
        assert_eq!(panel_id(PanelSide::Left, 0), Some("disk"));
        assert_eq!(panel_id(PanelSide::Left, 1), Some("workspace"));
        assert_eq!(panel_id(PanelSide::Left, 2), Some("notes"));
        assert_eq!(panel_id(PanelSide::Left, 3), Some("open"));
        assert_eq!(panel_id(PanelSide::Left, 4), Some("history"));
        assert_eq!(canonical_panel(PanelSide::Left, "", 3), "open");
        assert_eq!(panel_hint("disk"), "Проводник. Каталоги и файлы на диске");
        assert!(!panel_hint("notes").is_empty());
        assert_eq!(canonical_panel(PanelSide::Left, "open", 0), "open");
        assert_eq!(canonical_panel(PanelSide::Right, "nope", 9), "outline");
        let order = ordered_ids(
            PanelSide::Left,
            &["notes".to_string(), "notes".to_string(), "missing".to_string()],
        );
        assert_eq!(order[0], "notes");
        assert_eq!(order.len(), 5);
        assert_eq!(panel_icon("disk"), "system-file-manager-symbolic");
        assert_eq!(panel_icon("notes"), "document-edit-symbolic");
        assert!(!panel_icon("workspace").is_empty());
        assert!(order.iter().any(|id| id == "disk"));
    }
}
