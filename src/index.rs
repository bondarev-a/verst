use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::markdown;

const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", "dist", ".cache"];
const MAX_MD_FILES: usize = 20_000;
const MAX_SEARCH_BYTES: u64 = 512 * 1024;
const BINARY_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "ico", "pdf", "zip", "gz", "tgz", "bz2", "xz", "7z", "o",
    "a", "so", "dylib", "wasm", "mp3", "mp4", "ttf", "otf", "woff", "woff2", "class", "jar",
    "sqlite", "db",
];

#[derive(Debug, Clone, Default)]
pub struct LinkIndex {
    pub files: Vec<PathBuf>,
    pub outgoing: HashMap<PathBuf, Vec<PathBuf>>,
    pub incoming: HashMap<PathBuf, Vec<PathBuf>>,
    pub file_tags: HashMap<PathBuf, Vec<String>>,
    pub tag_files: HashMap<String, Vec<PathBuf>>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub path: PathBuf,
    pub line: usize,
    pub preview: String,
}

impl LinkIndex {
    pub fn incoming_of(&self, path: &Path) -> &[PathBuf] {
        self.outgoing_slice(&self.incoming, path)
    }

    /// Входящие из графа ссылок. Ключ в индексе нормализован, открытый путь может отличаться.
    pub fn incoming_edges(&self, path: &Path) -> Vec<PathBuf> {
        let direct = self.incoming_of(path);
        if !direct.is_empty() {
            return direct.to_vec();
        }
        let normalized = markdown::normalize(path.to_path_buf());
        if normalized.as_path() != path {
            return self.incoming_of(&normalized).to_vec();
        }
        Vec::new()
    }

    pub fn outgoing_of(&self, path: &Path) -> &[PathBuf] {
        self.outgoing_slice(&self.outgoing, path)
    }

    pub fn files_with_tag(&self, tag: &str) -> &[PathBuf] {
        const NONE: &[PathBuf] = &[];
        self.tag_files.get(tag).map(Vec::as_slice).unwrap_or(NONE)
    }

    /// Подменяет теги одного файла. Так открытый буфер попадает в поиск до полной переиндексации.
    pub fn set_file_tags(&mut self, path: &Path, tags: Vec<String>) {
        let old = self.file_tags.get(path).cloned().unwrap_or_default();
        if old == tags {
            return;
        }
        for tag in &old {
            if tags.contains(tag) {
                continue;
            }
            let Some(files) = self.tag_files.get_mut(tag) else {
                continue;
            };
            files.retain(|item| item != path);
            if files.is_empty() {
                self.tag_files.remove(tag);
            }
        }
        for tag in &tags {
            if old.contains(tag) {
                continue;
            }
            let files = self.tag_files.entry(tag.clone()).or_default();
            if !files.iter().any(|item| item == path) {
                files.push(path.to_path_buf());
                files.sort();
            }
        }
        if tags.is_empty() {
            self.file_tags.remove(path);
        } else {
            self.file_tags.insert(path.to_path_buf(), tags);
        }
    }

    /// Подменяет исходящие одного файла и входящие на его цели.
    /// Так открытый буфер попадает в панель до полной переиндексации.
    pub fn set_file_outgoing(&mut self, path: &Path, targets: Vec<PathBuf>) {
        let old = self.outgoing.get(path).cloned().unwrap_or_default();
        if old == targets {
            return;
        }
        for target in &old {
            if targets.iter().any(|item| item == target) {
                continue;
            }
            let Some(files) = self.incoming.get_mut(target) else {
                continue;
            };
            files.retain(|item| item != path);
            if files.is_empty() {
                self.incoming.remove(target);
            }
        }
        for target in &targets {
            if old.iter().any(|item| item == target) {
                continue;
            }
            let files = self.incoming.entry(target.clone()).or_default();
            if !files.iter().any(|item| item == path) {
                files.push(path.to_path_buf());
            }
        }
        if targets.is_empty() {
            self.outgoing.remove(path);
        } else {
            self.outgoing.insert(path.to_path_buf(), targets);
        }
    }

    fn outgoing_slice<'a>(
        &'a self,
        map: &'a HashMap<PathBuf, Vec<PathBuf>>,
        path: &Path,
    ) -> &'a [PathBuf] {
        if let Some(found) = map.get(path) {
            return found;
        }
        let normalized = markdown::normalize(path.to_path_buf());
        if normalized.as_path() != path {
            if let Some(found) = map.get(normalized.as_path()) {
                return found;
            }
        }
        &[]
    }
}

pub fn build_index(pin: &Path) -> LinkIndex {
    let mut files = Vec::new();
    let mut truncated = false;
    collect_md(pin, &mut files, &mut truncated);
    files.sort();
    let mut outgoing: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut incoming: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    let mut file_tags: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut tag_files: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for file in &files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let targets = outgoing_targets(&text, file, pin, &files);
        for target in &targets {
            incoming
                .entry(target.clone())
                .or_default()
                .push(file.clone());
        }
        if !targets.is_empty() {
            outgoing.insert(file.clone(), targets);
        }
        let tags = markdown::extract_tags(&text);
        if !tags.is_empty() {
            for tag in &tags {
                tag_files.entry(tag.clone()).or_default().push(file.clone());
            }
            file_tags.insert(file.clone(), tags);
        }
    }
    for paths in tag_files.values_mut() {
        paths.sort();
        paths.dedup();
    }
    LinkIndex {
        files,
        outgoing,
        incoming,
        file_tags,
        tag_files,
        truncated,
    }
}

fn collect_md(root: &Path, files: &mut Vec<PathBuf>, truncated: &mut bool) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if files.len() >= MAX_MD_FILES {
            *truncated = true;
            return;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && is_markdown(&path) {
                files.push(path);
                if files.len() >= MAX_MD_FILES {
                    *truncated = true;
                    return;
                }
            }
        }
    }
}

fn is_markdown(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("md" | "markdown")
    )
}

pub fn markdown_in(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut truncated = false;
    collect_md(root, &mut files, &mut truncated);
    files
}

pub fn is_within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

/// Пин, внутри которого разрешаются ссылки заметки.
/// Активный пин, если файл лежит в нём; иначе другой запиненный каталог с этим файлом.
pub fn link_scope<'a>(from: &Path, active: &'a Path, pins: &'a [PathBuf]) -> &'a Path {
    if is_within(active, from) {
        return active;
    }
    pins.iter()
        .find(|pin| is_within(pin, from))
        .map(PathBuf::as_path)
        .unwrap_or(active)
}

/// Локальные цели из текста заметки. Удалённые ссылки, код в ограждении и сам файл отбрасываются.
/// Цель остаётся, если файл существует внутри `scope`, даже когда это не Markdown.
pub fn outgoing_targets(
    text: &str,
    from: &Path,
    scope: &Path,
    markdown_files: &[PathBuf],
) -> Vec<PathBuf> {
    let from_norm = markdown::normalize(from.to_path_buf());
    let mut targets = Vec::new();
    for link in markdown::extract_links(text) {
        let Some(path) = resolve_outgoing(&link, &from_norm, scope, markdown_files) else {
            continue;
        };
        if path == from_norm
            || path == from
            || !target_allowed(scope, &path)
            || targets.contains(&path)
        {
            continue;
        }
        targets.push(path);
    }
    targets
}

fn resolve_outgoing(
    link: &markdown::RawLink,
    from: &Path,
    scope: &Path,
    files: &[PathBuf],
) -> Option<PathBuf> {
    match link.kind {
        markdown::LinkKind::Markdown => {
            let path = markdown::resolve_markdown(from, &link.target)?;
            locate_file(&path)
        }
        markdown::LinkKind::Wiki => {
            let matches = markdown::resolve_wiki(&link.target, scope, files);
            if matches.len() > 1 {
                return None;
            }
            if let Some(path) = matches.first() {
                if path.is_file() {
                    return Some(markdown::normalize(path.clone()));
                }
            }
            wiki_on_disk(&link.target, scope, from)
        }
    }
}

fn locate_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(markdown::normalize(path.to_path_buf()));
    }
    if path.extension().is_none() {
        for ext in ["md", "markdown"] {
            let candidate = path.with_extension(ext);
            if candidate.is_file() {
                return Some(markdown::normalize(candidate));
            }
        }
    }
    None
}

fn wiki_on_disk(name: &str, scope: &Path, from: &Path) -> Option<PathBuf> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let relative = Path::new(name);
    let mut candidates = Vec::new();
    if relative.is_absolute() {
        candidates.push(markdown::normalize(relative.to_path_buf()));
    } else {
        candidates.push(markdown::normalize(scope.join(relative)));
        if let Some(parent) = from.parent() {
            candidates.push(markdown::normalize(parent.join(relative)));
        }
    }
    for candidate in candidates {
        if let Some(path) = locate_file(&candidate) {
            if target_allowed(scope, &path) {
                return Some(path);
            }
        }
    }
    None
}

fn target_allowed(scope: &Path, path: &Path) -> bool {
    let Ok(rest) = path.strip_prefix(scope) else {
        return false;
    };
    !rest.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy();
        SKIP_DIRS.iter().any(|skip| *skip == name.as_ref())
    })
}

pub fn list_text_files(pin: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![pin.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if files.len() >= MAX_MD_FILES {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if SKIP_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() && !is_binary_name(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn is_binary_name(path: &Path) -> bool {
    if crate::kind::is_view_only(path) {
        return true;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .is_some_and(|ext| BINARY_EXT.contains(&ext.as_str()))
}

pub fn search_pin(pin: &Path, query: &str, limit: usize) -> Vec<SearchHit> {
    let query = query.trim();
    if query.is_empty() || limit == 0 {
        return Vec::new();
    }
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    let mut stack = vec![pin.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if hits.len() >= limit {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if hits.len() >= limit {
                break;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                if !SKIP_DIRS.contains(&name.as_ref()) {
                    stack.push(path);
                }
                continue;
            }
            if !file_type.is_file() || !searchable(&path) {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            if bytes.contains(&0) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let text_lower = text.to_lowercase();
            if !text_lower.contains(&needle) {
                continue;
            }
            for (line, raw) in text.lines().enumerate() {
                if raw.to_lowercase().contains(&needle) {
                    let mut preview = raw.trim().to_string();
                    if preview.chars().count() > 160 {
                        preview = preview.chars().take(160).collect();
                    }
                    hits.push(SearchHit {
                        path: path.clone(),
                        line,
                        preview,
                    });
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
        }
    }
    hits
}

fn searchable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() > MAX_SEARCH_BYTES {
        return false;
    }
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
    {
        Some(ext) if BINARY_EXT.iter().any(|item| *item == ext) => false,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_connects_markdown_and_wiki() {
        let root =
            std::env::temp_dir().join(format!("text_workspace_index_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("a.md"), "Ссылка [b](./nested/b.md) и [[c]]\n").unwrap();
        fs::write(root.join("nested/b.md"), "назад [a](../a.md)\n").unwrap();
        fs::write(root.join("c.md"), "пусто\n").unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/hidden.md"), "[[a]]\n").unwrap();

        let index = build_index(&root);
        assert!(index
            .files
            .iter()
            .all(|path| !path.components().any(|c| c.as_os_str() == ".git")));
        let a = root.join("a.md");
        let outgoing = index.outgoing_of(&a);
        assert!(outgoing.iter().any(|path| path.ends_with("nested/b.md")));
        assert!(outgoing.iter().any(|path| path.ends_with("c.md")));
        assert!(index
            .incoming_of(&root.join("c.md"))
            .iter()
            .any(|path| path.ends_with("a.md")));

        let hits = search_pin(&root, "пусто", 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].path.ends_with("c.md"));

        fs::write(
            root.join("a.md"),
            "---\ntags: [общая]\n---\n# Заголовок\n#уникальная\n",
        )
        .unwrap();
        fs::write(root.join("c.md"), "тоже #общая\n").unwrap();
        let index = build_index(&root);
        assert_eq!(index.files_with_tag("общая").len(), 2);
        assert_eq!(index.files_with_tag("уникальная").len(), 1);
        assert!(index.files_with_tag("заголовок").is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn outgoing_keeps_existing_files_and_skips_remote_code_and_self() {
        let root =
            std::env::temp_dir().join(format!("text_workspace_out_files_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("docs")).unwrap();
        let outside =
            std::env::temp_dir().join(format!("text_workspace_out_side_{}", std::process::id()));
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("ext.md"), "вне\n").unwrap();
        let agents = root.join("AGENTS.md");
        let body = format!(
            "Читать вместе с [`README.md`](README.md) и [`docs/library.md`](docs/library.md).\n\
             Скрипт [run.sh](run.sh) и [себя](AGENTS.md).\n\
             [сайт](https://example.com) и [вне]({}).\n\
             `[скрыто](hidden.sh)`\n\
             ```\n\
             [в коде](hidden.sh)\n\
             ```\n",
            outside.join("ext.md").display()
        );
        fs::write(&agents, body).unwrap();
        fs::write(root.join("README.md"), "readme\n").unwrap();
        fs::write(root.join("docs/library.md"), "lib\n").unwrap();
        fs::write(root.join("run.sh"), "echo\n").unwrap();
        fs::write(root.join("hidden.sh"), "no\n").unwrap();

        let index = build_index(&root);
        let outgoing = index.outgoing_of(&agents);
        assert!(outgoing.iter().any(|path| path.ends_with("README.md")));
        assert!(outgoing
            .iter()
            .any(|path| path.ends_with("docs/library.md")));
        assert!(outgoing.iter().any(|path| path.ends_with("run.sh")));
        assert_eq!(outgoing.len(), 3);
        assert!(outgoing.iter().all(|path| path != &agents));
        assert!(outgoing.iter().all(|path| !path.ends_with("hidden.sh")));
        assert!(outgoing.iter().all(|path| !path.ends_with("ext.md")));

        fs::write(root.join("c.md"), "новая\n").unwrap();
        let text = "См. [заметку](c.md)\n";
        let targets = outgoing_targets(text, &agents, &root, &index.files);
        let mut index = index;
        index.set_file_outgoing(&agents, targets);
        assert!(index
            .outgoing_of(&agents)
            .iter()
            .any(|path| path.ends_with("c.md")));
        assert!(index
            .incoming_of(&root.join("c.md"))
            .iter()
            .any(|path| path == &agents));
        assert!(index
            .incoming_of(&root.join("README.md"))
            .iter()
            .all(|path| path != &agents));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn note_outside_active_pin_lists_links_inside_its_own_pin() {
        let root =
            std::env::temp_dir().join(format!("text_workspace_out_pins_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let models = root.join("metamodels");
        let active = root.join("monorepo");
        fs::create_dir_all(models.join("docs")).unwrap();
        fs::create_dir_all(&active).unwrap();
        let agents = models.join("AGENTS.md");
        fs::write(
            &agents,
            "Читать вместе с [`README.md`](README.md) и [`docs/library.md`](docs/library.md).\n\
             [чужое](../monorepo/README.md)\n",
        )
        .unwrap();
        fs::write(models.join("README.md"), "a\n").unwrap();
        fs::write(models.join("docs/library.md"), "b\n").unwrap();
        fs::write(active.join("README.md"), "c\n").unwrap();
        fs::write(models.join("todo.txt"), "t\n").unwrap();
        fs::write(models.join("wiki.md"), "См. [[todo.txt]]\n").unwrap();

        let pins = vec![models.clone(), active.clone()];
        let scope = link_scope(&agents, &active, &pins);
        assert_eq!(scope, models.as_path());
        let text = fs::read_to_string(&agents).unwrap();
        let targets = outgoing_targets(&text, &agents, scope, &[]);
        assert!(targets
            .iter()
            .any(|path| path.ends_with("metamodels/README.md")));
        assert!(targets.iter().any(|path| path.ends_with("docs/library.md")));
        assert!(targets
            .iter()
            .all(|path| !path.ends_with("monorepo/README.md")));
        assert_eq!(targets.len(), 2);

        let active_index = build_index(&active);
        assert!(active_index.incoming_edges(&agents).is_empty());

        fs::write(models.join("README.md"), "См. [правила](AGENTS.md)\n").unwrap();
        let scope_index = build_index(&models);
        assert!(scope_index
            .incoming_edges(&agents)
            .iter()
            .any(|path| path.ends_with("README.md")));

        let wiki = fs::read_to_string(models.join("wiki.md")).unwrap();
        let wiki_files = markdown_in(&models);
        let wiki_targets = outgoing_targets(&wiki, &models.join("wiki.md"), &models, &wiki_files);
        assert!(wiki_targets.iter().any(|path| path.ends_with("todo.txt")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn outgoing_lookup_accepts_a_dot_segment() {
        let mut index = LinkIndex::default();
        let key = PathBuf::from("/tmp/notes/a.md");
        index
            .outgoing
            .insert(key, vec![PathBuf::from("/tmp/notes/b.md")]);
        let aliased = PathBuf::from("/tmp/notes/./a.md");
        assert_eq!(index.outgoing_of(&aliased).len(), 1);
    }
}
