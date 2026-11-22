use std::path::PathBuf;

/// Быстрая заметка: файл лежит в личном каталоге, а не в запиненном пространстве.
pub fn is_inbox(path: &std::path::Path) -> bool {
    path.starts_with(notes_dir())
}

/// Личный каталог быстрых заметок, отдельно от запиненного пространства.
pub fn notes_dir() -> PathBuf {
    data_dir().join("notes")
}

fn data_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("text_workspace");
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".local").join("share").join("text_workspace")
}

/// Предел длины имени файла заметки в байтах. В ext4 имя до 255 байт, а
/// кириллица — два байта на букву: заголовок длиннее ~127 букв давал ENAMETOOLONG.
const MAX_NAME_BYTES: usize = 200;

pub fn note_title(raw: &str) -> String {
    let mut title = strip_markdown_ext(raw.trim()).trim().to_string();
    title = title
        .replace(['/', '\\', '\0'], " ")
        .split_whitespace()
        .filter(|part| *part != "." && *part != "..")
        .collect::<Vec<_>>()
        .join(" ");
    if title.is_empty() || title == "." || title == ".." {
        "Заметка".to_string()
    } else {
        title
    }
}

pub fn note_body(raw: &str) -> String {
    format!("# {}\n\n", note_title(raw))
}

/// `.md` или `.markdown` в конце, в любом регистре (`Дело.MD`).
fn strip_markdown_ext(title: &str) -> &str {
    for ext in [".markdown", ".md"] {
        let Some(at) = title.len().checked_sub(ext.len()) else {
            continue;
        };
        if title.is_char_boundary(at) && title[at..].eq_ignore_ascii_case(ext) {
            return &title[..at];
        }
    }
    title
}

/// Имя `основа` + `хвост` не длиннее MAX_NAME_BYTES: основа обрезается по
/// границе символа, хвост (` 2.md`) остаётся целым.
fn fit_name(stem: &str, tail: &str) -> String {
    let mut end = stem.len().min(MAX_NAME_BYTES.saturating_sub(tail.len()));
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{tail}", stem[..end].trim_end())
}

/// Свободное имя файла для заметки. Никогда не возвращает занятое: номер
/// растёт, пока имя не освободится, а разные номера дают разные имена.
pub fn unique_note_name(raw: &str, existing: &[String]) -> String {
    let stem = note_title(raw);
    let taken: std::collections::HashSet<&str> = existing.iter().map(String::as_str).collect();
    let name = fit_name(&stem, ".md");
    if !taken.contains(name.as_str()) {
        return name;
    }
    let mut index = 2u64;
    loop {
        let name = fit_name(&stem, &format!(" {index}.md"));
        if !taken.contains(name.as_str()) {
            return name;
        }
        index += 1;
    }
}

pub fn note_label(source: &str, file_stem: &str) -> String {
    crate::markdown::headings(source)
        .into_iter()
        .map(|heading| heading.text)
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| file_stem.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteGroup {
    Flat,
    Created,
    Modified,
    Tags,
}

impl NoteGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flat => "flat",
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Tags => "tags",
        }
    }
}

pub fn note_group(raw: &str) -> NoteGroup {
    match raw {
        "created" => NoteGroup::Created,
        "modified" => NoteGroup::Modified,
        "tags" => NoteGroup::Tags,
        _ => NoteGroup::Flat,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DayGroup {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub indexes: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagGroup {
    pub tag: Option<String>,
    pub indexes: Vec<usize>,
}

/// Заметка с несколькими тегами попадает в каждую ветку. Без тега — одна ветка в конце.
pub fn group_by_tag(notes: &[Vec<String>]) -> Vec<TagGroup> {
    let mut tagged: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    let mut untagged = Vec::new();
    for (index, tags) in notes.iter().enumerate() {
        if tags.is_empty() {
            untagged.push(index);
            continue;
        }
        for tag in tags {
            let branch = tagged.entry(tag.clone()).or_default();
            if !branch.contains(&index) {
                branch.push(index);
            }
        }
    }
    let mut groups: Vec<TagGroup> = tagged
        .into_iter()
        .map(|(tag, indexes)| TagGroup {
            tag: Some(tag),
            indexes,
        })
        .collect();
    if !untagged.is_empty() {
        groups.push(TagGroup {
            tag: None,
            indexes: untagged,
        });
    }
    groups
}

/// `stamps` уже отсортированы: новые даты раньше. Одинаковый день остаётся одной веткой.
pub fn group_by_day(stamps: &[(i32, u32, u32)]) -> Vec<DayGroup> {
    let mut groups: Vec<DayGroup> = Vec::new();
    for (index, stamp) in stamps.iter().enumerate() {
        if let Some(last) = groups.last_mut() {
            if last.year == stamp.0 && last.month == stamp.1 && last.day == stamp.2 {
                last.indexes.push(index);
                continue;
            }
        }
        groups.push(DayGroup {
            year: stamp.0,
            month: stamp.1,
            day: stamp.2,
            indexes: vec![index],
        });
    }
    groups
}

pub fn has_tag(tags: &[String], filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(filter) => tags.iter().any(|tag| tag == filter),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_title_becomes_a_note_and_collisions_grow() {
        assert_eq!(note_title("  "), "Заметка");
        assert_eq!(note_title("../секрет"), "секрет");
        assert_eq!(note_body("Дело"), "# Дело\n\n");
        let existing = vec!["Заметка.md".to_string(), "Заметка 2.md".to_string()];
        assert_eq!(unique_note_name("", &existing), "Заметка 3.md");
        assert_eq!(unique_note_name("Дело.md", &[]), "Дело.md");
        assert!(has_tag(&["дом".to_string()], None));
        assert!(has_tag(&["дом".to_string()], Some("дом")));
        assert!(!has_tag(&["дом".to_string()], Some("работа")));
        assert_eq!(note_label("# Кухня\nтекст", "файл"), "Кухня");
        assert_eq!(note_label("без заголовка", "файл"), "файл");
        assert_eq!(note_group("created"), NoteGroup::Created);
        assert!(is_inbox(&notes_dir().join("дело.md")));
        assert!(!is_inbox(std::path::Path::new("/tmp/дело.md")));
        assert_eq!(note_group("tags"), NoteGroup::Tags);
        assert_eq!(NoteGroup::Tags.as_str(), "tags");
        assert_eq!(note_group("другое"), NoteGroup::Flat);
        let tags = group_by_tag(&[
            vec!["дом".to_string(), "работа".to_string()],
            vec!["дом".to_string()],
            vec![],
        ]);
        assert_eq!(tags.len(), 3);
        assert_eq!(tags[0].tag.as_deref(), Some("дом"));
        assert_eq!(tags[0].indexes, vec![0, 1]);
        assert_eq!(tags[1].tag.as_deref(), Some("работа"));
        assert_eq!(tags[1].indexes, vec![0]);
        assert_eq!(tags[2].tag, None);
        let groups = group_by_day(&[(2026, 10, 22), (2026, 10, 22), (2026, 10, 21), (0, 0, 0)]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].indexes, vec![0, 1]);
        assert_eq!(groups[1].day, 21);
        assert_eq!(groups[2].year, 0);
    }

    #[test]
    fn markdown_extension_is_stripped_in_any_case() {
        assert_eq!(note_title("Дело.MD"), "Дело");
        assert_eq!(note_title("Дело.Markdown"), "Дело");
        assert_eq!(unique_note_name("План.Md", &[]), "План.md");
        assert_eq!(note_title("Ёж.mdx"), "Ёж.mdx");
    }

    #[test]
    fn long_title_gives_a_short_enough_name() {
        let title = "Очень длинный заголовок заметки ".repeat(10);
        let name = unique_note_name(&title, &[]);
        assert!(name.len() <= MAX_NAME_BYTES, "{}", name.len());
        assert!(name.ends_with(".md"));
        assert!(name.starts_with("Очень длинный"));
        assert!(!name.contains(" .md"));
        let next = unique_note_name(&title, std::slice::from_ref(&name));
        assert!(next.len() <= MAX_NAME_BYTES);
        assert!(next.ends_with(" 2.md"));
        assert_ne!(next, name);
        // Заголовок в тексте заметки не обрезается.
        assert_eq!(note_body(&title), format!("# {}\n\n", title.trim()));
    }

    #[test]
    fn unique_name_is_never_taken() {
        let mut existing = vec!["Заметка.md".to_string()];
        for index in 2..=1500 {
            existing.push(format!("Заметка {index}.md"));
        }
        assert_eq!(unique_note_name("", &existing), "Заметка 1501.md");
    }
}
