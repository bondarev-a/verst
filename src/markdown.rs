use std::cell::OnceCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};

use pulldown_cmark::{html, CowStr, Event, LinkType, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub text: String,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Markdown,
    Wiki,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawLink {
    pub kind: LinkKind,
    pub target: String,
    pub line: usize,
}

/// Без метки порядка байтов (U+FEFF) в начале. Её оставляют редакторы
/// Windows; с ней первая строка `# Заголовок` или `---` не узнаётся.
/// Сам файл не меняется: метка снимается только при разборе.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
}

pub fn headings(source: &str) -> Vec<Heading> {
    let source = strip_bom(source);
    let mut out = Vec::new();
    let mut in_fence = false;
    // Строки шапки YAML пропускаются: `# комментарий` в ней не заголовок.
    let skip = match split_frontmatter(source) {
        (Some(front), _) => front.lines().count(),
        (None, _) => 0,
    };
    for (line, raw) in source.lines().enumerate().skip(skip) {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&hashes) {
            continue;
        }
        let rest = &trimmed[hashes..];
        if !rest.starts_with(' ') && !rest.starts_with('\t') {
            continue;
        }
        let text = rest.trim();
        if text.is_empty() {
            continue;
        }
        out.push(Heading {
            level: hashes as u8,
            text: text.to_string(),
            line,
        });
    }
    out
}

pub fn extract_links(source: &str) -> Vec<RawLink> {
    let source = strip_bom(source);
    let mut out = Vec::new();
    let mut in_fence = false;
    for (line_index, raw) in source.lines().enumerate() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        scan_line(raw, line_index, &mut out);
    }
    out
}

fn scan_line(line: &str, line_index: usize, out: &mut Vec<RawLink>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut code = false;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            code = !code;
            i += 1;
            continue;
        }
        if code || bytes[i] != b'[' {
            i += 1;
            continue;
        }
        if let Some(link) = wiki_at(line, i) {
            // `[[#Раздел]]` ведёт в тот же файл: в графе это не ребро.
            if !link.parts.target.is_empty() {
                out.push(RawLink {
                    kind: LinkKind::Wiki,
                    target: link.parts.target.to_string(),
                    line: line_index,
                });
            }
            i = link.end;
            continue;
        }
        if i == 0 || bytes[i - 1] != b'!' {
            if let Some(close) = find_byte(bytes, i + 1, b']') {
                if bytes.get(close + 1) == Some(&b'(') {
                    if let Some(url_end) = find_byte(bytes, close + 2, b')') {
                        let target = line[close + 2..url_end].trim().to_string();
                        if is_local_markdown_target(&target) {
                            out.push(RawLink {
                                kind: LinkKind::Markdown,
                                target,
                                line: line_index,
                            });
                        }
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
}

/// Части вики-ссылки `[[цель#раздел|подпись]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WikiParts<'a> {
    /// Всё до `|`: цель вместе с разделом, как написано.
    pub link: &'a str,
    /// Имя заметки или путь. Пусто у `[[#Раздел]]`: это ссылка на текущий файл.
    pub target: &'a str,
    /// Раздел после `#`: заголовок или `^блок`.
    pub fragment: Option<&'a str>,
    /// Подпись после `|`.
    pub alias: Option<&'a str>,
}

/// Вики-ссылка в тексте: смещения в байтах.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WikiLink<'a> {
    /// Начало `[[`.
    pub start: usize,
    /// Байт после `]]`.
    pub end: usize,
    /// Начало видимой части: подписи, если она есть, иначе цели.
    pub shown: usize,
    pub parts: WikiParts<'a>,
}

/// Разбор внутренней части `цель#раздел|подпись` по правилам Obsidian.
/// Общий для предпросмотра, индекса ссылок и разметки редактора.
/// `None`, если в цели есть скобки или нет ни имени, ни раздела.
pub fn wiki_parts(inner: &str) -> Option<WikiParts<'_>> {
    let (link, alias) = match inner.split_once('|') {
        Some((link, alias)) => (link, Some(alias.trim()).filter(|alias| !alias.is_empty())),
        None => (inner, None),
    };
    if link.contains(['[', ']', '\n']) {
        return None;
    }
    // В таблицах Obsidian пишет `[[цель\|подпись]]`: обратная косая черта
    // экранирует `|` для таблицы и к имени не относится.
    let link = link.trim().trim_end_matches('\\').trim_end();
    let (target, fragment) = match link.split_once('#') {
        Some((target, fragment)) => (
            target.trim(),
            Some(fragment.trim()).filter(|fragment| !fragment.is_empty()),
        ),
        None => (link, None),
    };
    if target.is_empty() && fragment.is_none() {
        return None;
    }
    Some(WikiParts {
        link,
        target,
        fragment,
        alias,
    })
}

/// Вики-ссылка, которая начинается ровно с `[[` в `text[start..]`.
/// Ссылка кончается первой `]]` на той же строке; `[` внутри не бывает,
/// так что `[[a[b]]` — не ссылка ни в одном из разборов.
pub fn wiki_at(text: &str, start: usize) -> Option<WikiLink<'_>> {
    let rest = text.get(start..)?;
    if !rest.starts_with("[[") {
        return None;
    }
    let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
    let close = line[2..].find("]]")? + 2;
    let inner = &line[2..close];
    if inner.contains('[') {
        return None;
    }
    let parts = wiki_parts(inner)?;
    let shown = match inner.find('|') {
        Some(bar) if parts.alias.is_some() => start + 2 + bar + 1,
        _ => start + 2,
    };
    Some(WikiLink {
        start,
        end: start + close + 2,
        shown,
        parts,
    })
}

fn is_local_markdown_target(target: &str) -> bool {
    let target = target.trim();
    if target.is_empty() || target.starts_with('#') {
        return false;
    }
    let path = target.split(['#', '?']).next().unwrap_or(target).trim();
    if path.is_empty() {
        return false;
    }
    if let Some((scheme, _)) = path.split_once(':') {
        if scheme.chars().all(|c| c.is_ascii_alphabetic()) && !scheme.is_empty() {
            return false;
        }
    }
    true
}

fn find_byte(bytes: &[u8], from: usize, needle: u8) -> Option<usize> {
    bytes
        .get(from..)?
        .iter()
        .position(|byte| *byte == needle)
        .map(|i| i + from)
}

pub fn normalize(path: PathBuf) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn resolve_markdown(from: &Path, raw: &str) -> Option<PathBuf> {
    if !is_local_markdown_target(raw) {
        return None;
    }
    let raw = raw.split(['#', '?']).next().unwrap_or(raw).trim();
    let base = from.parent().unwrap_or(Path::new(""));
    Some(normalize(base.join(raw)))
}

pub fn resolve_wiki(name: &str, pin: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    WikiLookup::new(files).resolve(name, pin)
}

/// Поиск файлов по имени из вики-ссылки. Строится один раз на проход
/// (предпросмотр, индекс), а не перебирает все файлы на каждую ссылку.
/// Имя сравнивается без учёта регистра, как в Obsidian.
pub struct WikiLookup<'a> {
    files: &'a [PathBuf],
    by_stem: OnceCell<HashMap<String, Vec<usize>>>,
    by_name: OnceCell<HashMap<String, Vec<usize>>>,
    by_path: OnceCell<HashMap<String, Vec<usize>>>,
}

impl<'a> WikiLookup<'a> {
    pub fn new(files: &'a [PathBuf]) -> Self {
        Self {
            files,
            by_stem: OnceCell::new(),
            by_name: OnceCell::new(),
            by_path: OnceCell::new(),
        }
    }

    /// `name` — цель ссылки; раздел и подпись отбрасываются.
    pub fn resolve(&self, name: &str, pin: &Path) -> Vec<PathBuf> {
        let Some(parts) = wiki_parts(name) else {
            return Vec::new();
        };
        let target = parts.target;
        if target.is_empty() {
            return Vec::new();
        }
        let (map, key) = if target.contains('/') {
            let mut relative = PathBuf::from(target);
            if relative.extension().is_none() {
                relative.set_extension("md");
            }
            let candidate = normalize(pin.join(relative));
            let map = self.by_path.get_or_init(|| {
                self.group(|path| {
                    Some(normalize(path.to_path_buf()).to_string_lossy().into_owned())
                })
            });
            (map, candidate.to_string_lossy().to_lowercase())
        } else if has_markdown_ext(Path::new(target)) {
            let map = self.by_name.get_or_init(|| {
                self.group(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                })
            });
            (map, target.to_lowercase())
        } else {
            let map = self.by_stem.get_or_init(|| {
                self.group(|path| {
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(str::to_string)
                })
            });
            (map, target.to_lowercase())
        };
        map.get(&key)
            .map(|found| found.iter().map(|at| self.files[*at].clone()).collect())
            .unwrap_or_default()
    }

    fn group(&self, key: impl Fn(&Path) -> Option<String>) -> HashMap<String, Vec<usize>> {
        let mut map: HashMap<String, Vec<usize>> = HashMap::new();
        for (at, path) in self.files.iter().enumerate() {
            if let Some(key) = key(path) {
                map.entry(key.to_lowercase()).or_default().push(at);
            }
        }
        map
    }
}

/// `.md` или `.markdown` в любом регистре.
pub fn has_markdown_ext(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

pub enum LocalLink {
    Open { path: PathBuf, line: Option<usize> },
    Missing { label: String },
    Pick { name: String },
    Tag { name: String },
}

pub fn render_html(
    source: &str,
    from: &Path,
    pin: Option<&Path>,
    files: &[PathBuf],
    href: &impl Fn(LocalLink) -> String,
    missing_prefix: &str,
) -> String {
    let source = strip_bom(source);
    let rewriter = Rewriter {
        from,
        source,
        pin,
        lookup: WikiLookup::new(files),
        href,
    };
    let rewritten = rewriter.rewrite(source);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    // Шапка YAML (`---` … `---` в начале файла) — метаданные, а не линия и
    // заголовок второго уровня. В HTML она не выводится, как в Obsidian.
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    // Номер строки события — двоичным поиском по началам строк. Подсчёт `\n`
    // с начала файла на каждое событие был квадратичным: 200 мс на 200 КБ.
    let newlines: Vec<usize> = rewritten
        .bytes()
        .enumerate()
        .filter(|(_, byte)| *byte == b'\n')
        .map(|(index, _)| index)
        .collect();
    let parser = Parser::new_ext(&rewritten, options).into_offset_iter();
    let mut events = Vec::new();
    let mut seen_line = None;
    let mut in_code = false;
    for (event, range) in parser {
        let event = rewrite_link_event(event, from, href);
        let line = newlines.partition_point(|at| *at < range.start);
        if !in_code && seen_line != Some(line) {
            events.push(Event::Html(CowStr::from(format!(
                r#"<span id="twl{line}"></span>"#
            ))));
            seen_line = Some(line);
        }
        // Внутри кода и шапки якоря строк не вставляются: в коде они стали бы
        // текстом, а шапку писатель HTML не выводит, кроме сырого HTML.
        if matches!(
            event,
            Event::Start(Tag::CodeBlock(_)) | Event::Start(Tag::MetadataBlock(_))
        ) {
            in_code = true;
        }
        let ends_code = matches!(
            event,
            Event::End(TagEnd::CodeBlock) | Event::End(TagEnd::MetadataBlock(_))
        );
        events.push(event);
        if ends_code {
            in_code = false;
        }
    }
    let mut body = String::new();
    html::push_html(&mut body, events.into_iter());
    let body = strip_remote_images(&body);
    let missing_css = if missing_prefix.is_empty() {
        String::new()
    } else {
        format!(r#"a[href^="{missing_prefix}"] {{ text-decoration: underline dotted; }}"#)
    };
    format!(
        r#"<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<style>
  :root {{ color-scheme: light dark; }}
  body {{ font-family: sans-serif; line-height: 1.45; margin: 16px; }}
  pre {{ overflow: auto; padding: 8px; background: rgba(127,127,127,0.12); }}
  code {{ font-family: monospace; }}
  {missing_css}
  img {{ max-width: 100%; }}
  [id^="twl"] {{ display: inline-block; width: 0; height: 1em; vertical-align: top; }}
</style>
</head>
<body>
{body}
</body>
</html>"#
    )
}

#[cfg(test)]
fn line_at_byte(source: &str, byte: usize) -> usize {
    let end = byte.min(source.len());
    source[..end].bytes().filter(|byte| *byte == b'\n').count()
}

/// Переписывает вики-ссылки и теги в обычные ссылки Markdown до разбора.
struct Rewriter<'a, F: Fn(LocalLink) -> String> {
    from: &'a Path,
    /// Текст текущего файла: по нему ищется заголовок у `[[#Раздел]]`.
    source: &'a str,
    pin: Option<&'a Path>,
    lookup: WikiLookup<'a>,
    href: &'a F,
}

impl<F: Fn(LocalLink) -> String> Rewriter<'_, F> {
    fn rewrite(&self, source: &str) -> String {
        let (front, body) = split_frontmatter(source);
        let mut out = String::with_capacity(source.len() + 64);
        if let Some(front) = front {
            out.push_str(front);
        }
        let mut literal = Literal::new(literal_ranges(body, false));
        let mut base = 0;
        for piece in body.split_inclusive('\n') {
            let line = piece.trim_end_matches(['\n', '\r']);
            self.rewrite_line(line, base, &mut literal, &mut out);
            out.push_str(&piece[line.len()..]);
            base += piece.len();
        }
        out
    }

    /// `base` — смещение строки в теле заметки, по нему сверяется `literal`.
    fn rewrite_line(&self, line: &str, base: usize, literal: &mut Literal, out: &mut String) {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            // Код и сырой HTML переносятся как есть: там нет ни ссылок, ни тегов.
            if let Some(end) = literal.covering(base + i) {
                let stop = (end - base).min(line.len());
                out.push_str(&line[i..stop]);
                i = stop;
                continue;
            }
            if bytes[i] == b'[' {
                if let Some(link) = wiki_at(line, i) {
                    out.push_str(&self.wiki_markdown(&link.parts));
                    i = link.end;
                    continue;
                }
                if i == 0 || bytes[i - 1] != b'!' {
                    out.push('[');
                    i += 1;
                    if let Some(end) = skip_link_label_and_dest(bytes, i) {
                        out.push_str(&line[i..end]);
                        i = end;
                    }
                    continue;
                }
            }
            if let Some(tag) = tag_at(line, i) {
                out.push_str(&format!(
                    "[{}]({})",
                    &line[i..tag.end],
                    (self.href)(LocalLink::Tag { name: tag.name })
                ));
                i = tag.end;
                continue;
            }
            let ch = line[i..].chars().next().unwrap_or(' ');
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    fn wiki_markdown(&self, parts: &WikiParts) -> String {
        let target = parts.target;
        let link = if target.is_empty() {
            LocalLink::Open {
                path: self.from.to_path_buf(),
                line: parts
                    .fragment
                    .and_then(|fragment| fragment_line(self.source, fragment)),
            }
        } else {
            match self.pin {
                Some(pin) => {
                    let matches = self.lookup.resolve(target, pin);
                    match matches.as_slice() {
                        [only] => LocalLink::Open {
                            path: only.clone(),
                            line: parts
                                .fragment
                                .and_then(|fragment| heading_line(only, fragment)),
                        },
                        [] => LocalLink::Missing {
                            label: target.to_string(),
                        },
                        _ => LocalLink::Pick {
                            name: target.to_string(),
                        },
                    }
                }
                None => LocalLink::Missing {
                    label: target.to_string(),
                },
            }
        };
        format!(
            "[{}]({})",
            escape_label(&wiki_label(parts)),
            (self.href)(link)
        )
    }
}

/// Видимый текст вики-ссылки: подпись, иначе «Заметка > Раздел», как в Obsidian.
fn wiki_label(parts: &WikiParts) -> String {
    if let Some(alias) = parts.alias {
        return alias.to_string();
    }
    match (parts.target, parts.fragment) {
        ("", Some(fragment)) => fragment.to_string(),
        (target, Some(fragment)) => format!("{target} > {fragment}"),
        (target, None) => target.to_string(),
    }
}

/// Подпись вставляется в `[подпись](адрес)` буквально: скобки, `*`, `` ` ``
/// и прочая пунктуация ASCII экранируются, чтобы не сломать ссылку.
fn escape_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch.is_ascii_punctuation() {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Теги заметки: `#тег` в тексте и `tags:` в шапке YAML. Один и тот же тег в обоих местах совпадает.
pub fn extract_tags(source: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let (front, body) = split_frontmatter(source);
    if let Some(front) = front {
        collect_frontmatter_tags(front, &mut tags);
    }
    let mut literal = Literal::new(literal_ranges(body, true));
    let mut i = 0;
    while let Some(found) = body[i..].find(['#', '[']) {
        let at = i + found;
        if let Some(end) = literal.covering(at) {
            i = end.max(at + 1);
            continue;
        }
        // `[[#Раздел]]` и `[[Заметка#Раздел]]` — ссылки, а не теги.
        if body.as_bytes()[at] == b'[' {
            i = wiki_at(body, at).map_or(at + 1, |link| link.end);
            continue;
        }
        if let Some(tag) = tag_at(body, at) {
            push_tag(&mut tags, tag.name);
            i = tag.end;
            continue;
        }
        i = at + 1;
    }
    tags
}

/// Участки тела заметки, где `#` и `[[` — просто текст: код строчный,
/// блоком в ограде и с отступом, сырой HTML (`<a href="#x">`). С `links`
/// ещё и обычные ссылки целиком (`[текст](#раздел)`). Разбор тот же, что у
/// предпросмотра, так что границы кода совпадают с показанными.
/// Участки отсортированы и не пересекаются.
fn literal_ranges(body: &str, links: bool) -> Vec<Range<usize>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(body, options).into_offset_iter() {
        let literal = match event {
            Event::Start(Tag::CodeBlock(_))
            | Event::Start(Tag::HtmlBlock)
            | Event::Code(_)
            | Event::Html(_)
            | Event::InlineHtml(_) => true,
            Event::Start(Tag::Link { .. }) => links,
            _ => false,
        };
        if literal && range.start < range.end {
            ranges.push(range);
        }
    }
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

/// Проход по участкам `literal_ranges` с растущими смещениями: каждая
/// проверка — без пересчёта строки с начала.
struct Literal {
    ranges: Vec<Range<usize>>,
    at: usize,
}

impl Literal {
    fn new(ranges: Vec<Range<usize>>) -> Self {
        Self { ranges, at: 0 }
    }

    /// Конец участка, внутри которого лежит `pos`. Смещения не убывают.
    fn covering(&mut self, pos: usize) -> Option<usize> {
        while self
            .ranges
            .get(self.at)
            .is_some_and(|range| range.end <= pos)
        {
            self.at += 1;
        }
        self.ranges
            .get(self.at)
            .filter(|range| range.start <= pos)
            .map(|range| range.end)
    }
}

pub fn canonical_tag(raw: &str) -> Option<String> {
    let raw = raw
        .trim()
        .trim_matches(|ch| matches!(ch, '"' | '\'' | '«' | '»'));
    let raw = raw.strip_prefix('#').unwrap_or(raw).trim();
    if !tag_body_ok(raw) {
        return None;
    }
    Some(raw.to_lowercase())
}

struct FoundTag {
    name: String,
    end: usize,
}

/// Тег `#имя` ровно в `index`. Код и HTML отсекает вызывающий по `literal_ranges`.
fn tag_at(line: &str, index: usize) -> Option<FoundTag> {
    if line.as_bytes().get(index) != Some(&b'#') {
        return None;
    }
    if line[..index].chars().next_back().is_some_and(tag_glue) {
        return None;
    }
    let body_start = index + 1;
    let body_len: usize = line[body_start..]
        .chars()
        .take_while(|ch| is_tag_char(*ch))
        .map(char::len_utf8)
        .sum();
    if body_len == 0 {
        return None;
    }
    let end = body_start + body_len;
    let name = canonical_tag(&line[body_start..end])?;
    Some(FoundTag { name, end })
}

/// Знак перед `#`, с которым это не тег: слово (`C#`), адрес (`a.md#x`)
/// или экранирование (`\#`).
fn tag_glue(ch: char) -> bool {
    ch.is_alphanumeric()
        || matches!(
            ch,
            '_' | '#' | '/' | '.' | ':' | '?' | '&' | '=' | '-' | '\\'
        )
}

fn is_tag_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/')
}

fn tag_body_ok(body: &str) -> bool {
    if body.is_empty() || body.chars().count() > 80 {
        return false;
    }
    // Как в Obsidian: `#123` — номер, а не тег; нужен хотя бы один не цифровой знак.
    if body.chars().all(|ch| ch.is_numeric() || ch == '/') {
        return false;
    }
    body.split('/').all(|part| {
        let mut chars = part.chars();
        match chars.next() {
            Some(first) if first.is_alphanumeric() || first == '_' => {}
            _ => return false,
        }
        chars.all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '-')
    })
}

fn push_tag(tags: &mut Vec<String>, tag: String) {
    if !tags.contains(&tag) {
        tags.push(tag);
    }
}

fn split_frontmatter(source: &str) -> (Option<&str>, &str) {
    let source = strip_bom(source);
    let Some(rest) = source.strip_prefix("---") else {
        return (None, source);
    };
    let Some(rest) = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))
    else {
        return (None, source);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\r', '\n']);
        if bare == "---" {
            let delimiter = source.len() - rest.len();
            let end = delimiter + offset + line.len();
            return (Some(&source[..end]), &source[end..]);
        }
        offset += line.len();
        if offset > 16_384 {
            break;
        }
    }
    (None, source)
}

fn collect_frontmatter_tags(front: &str, tags: &mut Vec<String>) {
    let mut lines = front.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = strip_yaml_comment(line.trim());
        let Some(rest) = trimmed.strip_prefix("tags:") else {
            continue;
        };
        let rest = strip_yaml_comment(rest);
        if rest.is_empty() {
            while let Some(next) = lines.peek() {
                let item = next.trim();
                let Some(item) = item.strip_prefix("- ") else {
                    break;
                };
                if let Some(tag) = canonical_tag(item) {
                    push_tag(tags, tag);
                }
                lines.next();
            }
        } else if let Some(inner) = rest
            .strip_prefix('[')
            .and_then(|value| value.strip_suffix(']'))
        {
            for tag in split_tag_list(inner) {
                push_tag(tags, tag);
            }
        } else {
            for tag in split_tag_list(rest) {
                push_tag(tags, tag);
            }
        }
    }
}

fn strip_yaml_comment(raw: &str) -> &str {
    let mut quote = None;
    for (index, ch) in raw.char_indices() {
        if let Some(mark) = quote {
            if ch == mark {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '#' => return raw[..index].trim(),
            _ => {}
        }
    }
    raw.trim()
}

fn split_tag_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for ch in raw.chars() {
        if let Some(mark) = quote {
            if ch == mark {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            ',' => {
                if let Some(tag) = canonical_tag(&current) {
                    out.push(tag);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if let Some(tag) = canonical_tag(&current) {
        out.push(tag);
    }
    out
}

fn skip_link_label_and_dest(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    let mut depth = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    if bytes.get(i + 1) == Some(&b'(') {
                        i += 2;
                        while i < bytes.len() && bytes[i] != b')' {
                            i += 1;
                        }
                        if i < bytes.len() {
                            i += 1;
                        }
                    } else {
                        i += 1;
                    }
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

pub fn encode_path(path: &str) -> String {
    let mut out = String::new();
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'/' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub fn decode_path(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return None;
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let value = u8::from_str_radix(hex, 16).ok()?;
            out.push(value);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn rewrite_link_event<'a>(
    event: Event<'a>,
    from: &Path,
    href: &impl Fn(LocalLink) -> String,
) -> Event<'a> {
    match event {
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) if link_type != LinkType::Email => {
            if let Some(link) = classify_local(&dest_url, from) {
                Event::Start(Tag::Link {
                    link_type,
                    dest_url: CowStr::from(href(link)),
                    title,
                    id,
                })
            } else {
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                })
            }
        }
        other => other,
    }
}

fn classify_local(dest: &str, from: &Path) -> Option<LocalLink> {
    if !is_local_markdown_target(dest) {
        return None;
    }
    let (raw_path, fragment) = split_target(dest);
    let decoded = decode_path(raw_path).unwrap_or_else(|| raw_path.to_string());
    let base = from.parent().unwrap_or(Path::new(""));
    let resolved = normalize(base.join(decoded));
    let Some(file) = existing_file(&resolved) else {
        return Some(LocalLink::Missing {
            label: resolved.to_string_lossy().into_owned(),
        });
    };
    let fragment =
        fragment.and_then(|value| decode_path(value).or_else(|| Some(value.to_string())));
    let line = fragment.and_then(|value| heading_line(&file, &value));
    Some(LocalLink::Open { path: file, line })
}

fn split_target(raw: &str) -> (&str, Option<&str>) {
    let raw = raw.trim();
    let (path, rest) = raw.split_once('#').unwrap_or((raw, ""));
    let path = path.split('?').next().unwrap_or(path).trim();
    let fragment = rest.split('?').next().unwrap_or("").trim();
    if fragment.is_empty() {
        (path, None)
    } else {
        (path, Some(fragment))
    }
}

fn existing_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if path.extension().is_none() {
        for ext in ["md", "markdown"] {
            let candidate = path.with_extension(ext);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn heading_line(path: &Path, fragment: &str) -> Option<usize> {
    if !has_markdown_ext(path) {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 1_048_576 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    fragment_line(&text, fragment)
}

/// Строка раздела `#Заголовок` или блока `#^метка` в тексте заметки.
/// Заголовок сравнивается без учёта регистра или по слагу; у вложенного
/// `#Глава#Часть` берётся последний.
pub fn fragment_line(text: &str, fragment: &str) -> Option<usize> {
    let fragment = fragment.trim();
    if let Some(block) = fragment.strip_prefix('^') {
        let mark = format!("^{}", block.trim());
        let spaced = format!(" {mark}");
        return strip_bom(text).lines().position(|line| {
            let line = line.trim_end();
            line == mark || line.ends_with(&spaced)
        });
    }
    let wanted = fragment.rsplit('#').next().unwrap_or(fragment).trim();
    if wanted.is_empty() {
        return None;
    }
    let lower = wanted.to_lowercase();
    let want = slug(wanted);
    headings(text)
        .into_iter()
        .find(|heading| {
            heading.text.to_lowercase() == lower
                || (!want.is_empty() && slug(&heading.text) == want)
        })
        .map(|heading| heading.line)
}

pub fn heading_slug(text: &str) -> String {
    slug(text)
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

fn strip_remote_images(html: &str) -> String {
    html.replace("src=\"http://", "src=\"blocked:")
        .replace("src=\"https://", "src=\"blocked:")
        .replace("src='http://", "src='blocked:")
        .replace("src='https://", "src='blocked:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_lookup_matches_counting() {
        let text = "# Заголовок\n\nабзац\r\nещё\n\n| а | б |\n|---|---|\n| 1 | 2 |\nконец";
        let newlines: Vec<usize> = text
            .bytes()
            .enumerate()
            .filter(|(_, byte)| *byte == b'\n')
            .map(|(index, _)| index)
            .collect();
        for byte in (0..=text.len()).filter(|b| text.is_char_boundary(*b)) {
            assert_eq!(
                newlines.partition_point(|at| *at < byte),
                line_at_byte(text, byte)
            );
        }
    }

    #[test]
    fn heading_slug_keeps_numbers_and_words() {
        assert_eq!(heading_slug("1. Как устроена CRM"), "1-как-устроена-crm");
    }

    #[test]
    fn headings_skip_fences() {
        let source = "# Видно\n```\n# Скрыто\n```\n## Ещё\n";
        let found = headings(source);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "Видно");
        assert_eq!(found[0].line, 0);
        assert_eq!(found[1].level, 2);
    }

    #[test]
    fn links_ignore_remote_and_code() {
        let source =
            "См. [локальное](./note.md) и [сайт](https://example.com).\n`[[код]]`\n[[заметка]]\n";
        let links = extract_links(source);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].kind, LinkKind::Markdown);
        assert_eq!(links[0].target, "./note.md");
        assert_eq!(links[1].kind, LinkKind::Wiki);
        assert_eq!(links[1].target, "заметка");
    }

    #[test]
    fn wiki_alias_uses_target_before_bar() {
        let source = "[[имя|подпись]]";
        let links = extract_links(source);
        assert_eq!(links[0].target, "имя");
    }

    #[test]
    fn relative_link_resolves_against_file_dir() {
        let from = Path::new("/repo/docs/index.md");
        let resolved = resolve_markdown(from, "../readme.md#часть").unwrap();
        assert_eq!(resolved, PathBuf::from("/repo/readme.md"));
    }

    #[test]
    fn wiki_unique_stem_and_ambiguous() {
        let pin = Path::new("/repo");
        let files = vec![
            PathBuf::from("/repo/a.md"),
            PathBuf::from("/repo/nested/a.md"),
            PathBuf::from("/repo/nested/b.md"),
        ];
        assert_eq!(resolve_wiki("b", pin, &files).len(), 1);
        assert_eq!(resolve_wiki("a", pin, &files).len(), 2);
        assert_eq!(
            resolve_wiki("nested/b.md", pin, &files),
            vec![PathBuf::from("/repo/nested/b.md")]
        );
    }

    fn href(link: LocalLink) -> String {
        match link {
            LocalLink::Open { path, line } => format!(
                "open:{}:{}",
                path.display(),
                line.map(|value| value.to_string()).unwrap_or_default()
            ),
            LocalLink::Missing { label } => format!("missing:{label}"),
            LocalLink::Pick { name } => format!("pick:{name}"),
            LocalLink::Tag { name } => format!("tag:{name}"),
        }
    }

    #[test]
    fn tags_come_from_hash_and_frontmatter_but_not_headings_or_code() {
        let source = "\
---
tags:
  - Альфа
  - \"#бета\"
title: Заметка
---
# Заголовок
Текст #альфа и C#язык и file.md#часть.
`#скрыто`
```
#нет
```
(#гамма)
";
        assert_eq!(
            extract_tags(source),
            vec!["альфа".to_string(), "бета".to_string(), "гамма".to_string()]
        );
    }

    #[test]
    fn tags_skip_html_indented_code_inline_code_and_numbers() {
        let source = "\
Текст #есть и <a href=\"#якорь\">ссылка</a>.
``код с ` и #внутри`` и `#строчный` и #после
Номер #123 и #2024/10, но #2024год и #п1.
[ссылка](#раздел) и [[#Раздел]] и \\#экран

    #отступ в коде

- пункт

    #продолжение пункта
";
        assert_eq!(
            extract_tags(source),
            vec![
                "есть".to_string(),
                "после".to_string(),
                "2024год".to_string(),
                "п1".to_string(),
                "продолжение".to_string(),
            ]
        );
        let html = render_html(source, Path::new("/repo/index.md"), None, &[], &href, "");
        assert!(html.contains(r##"<a href="#якорь">"##));
        assert!(!html.contains(&format!("tag:{}", encode_path("якорь"))));
        assert!(html.contains(&format!("tag:{}", encode_path("после"))));
        assert!(!html.contains(&format!("tag:{}", encode_path("внутри"))));
        assert!(!html.contains(&format!("tag:{}", encode_path("отступ"))));
        assert!(!html.contains("tag:123"));
    }

    #[test]
    fn long_line_with_many_backticks_and_hashes() {
        let line = "`код` #тег ".repeat(20_000);
        assert_eq!(extract_tags(&line), vec!["тег".to_string()]);
        let html = render_html(&line, Path::new("/repo/index.md"), None, &[], &href, "");
        assert!(html.contains(&format!("tag:{}", encode_path("тег"))));
    }

    #[test]
    fn preview_turns_a_tag_into_a_link() {
        let html = render_html(
            "См. #Тема и [файл](note.md#часть).",
            Path::new("/repo/index.md"),
            None,
            &[],
            &href,
            "missing:",
        );
        assert!(html.contains(&format!("tag:{}", encode_path("тема"))));
        assert!(html.contains("#Тема"));
        assert!(!html.contains("tag:часть"));
    }

    #[test]
    fn html_points_unique_wiki_at_file() {
        let pin = Path::new("/repo");
        let files = vec![PathBuf::from("/repo/заметка.md")];
        let html = render_html(
            "[[заметка]]",
            Path::new("/repo/index.md"),
            Some(pin),
            &files,
            &href,
            "missing:",
        );
        assert!(html.contains("open:"));
        assert!(html.contains("заметка"));
        assert!(!html.contains("twopen:"));
        assert!(!html.contains("https://"));
    }

    #[test]
    fn relative_link_opens_neighbor_file() {
        let dir = std::env::temp_dir().join(format!("verst_links_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("note.md");
        std::fs::write(&note, "# Часть\nтекст\n").unwrap();
        let from = dir.join("index.md");
        let html = render_html(
            "См. [заметку](note.md#часть) и [сайт](https://example.com).",
            &from,
            None,
            &[],
            &href,
            "missing:",
        );
        assert!(html.contains(&format!("open:{}:0", note.display())));
        assert!(html.contains("https://example.com"));
        let marked = render_html("до\n```\nкод\n```\n", &from, None, &[], &href, "");
        assert!(marked.contains(r#"id="twl0""#));
        assert!(marked.contains(r#"id="twl1""#));
        assert!(!marked.contains(r#"id="twl2""#));
        assert!(!html.contains("href=\"note.md\""));
        assert!(!html.contains("twopen:"));
        let missing = render_html("[нет](./missing.md)", &from, None, &[], &href, "missing:");
        assert!(missing.contains("missing:"));
        assert!(!missing.contains("twmissing:"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn frontmatter_is_not_a_heading() {
        let source = "---\ntags: [проект]\n# комментарий\n---\n# Заголовок\nтекст\n";
        let html = render_html(source, Path::new("/repo/index.md"), None, &[], &href, "");
        assert!(!html.contains("<hr"));
        assert!(!html.contains("<h2"));
        assert!(!html.contains("проект"));
        assert!(html.contains("<h1>Заголовок</h1>"));
        assert!(html.contains(r#"id="twl4""#));
        let found = headings(source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "Заголовок");
        assert_eq!(found[0].line, 4);
        assert_eq!(extract_tags(source), vec!["проект".to_string()]);
    }

    #[test]
    fn bom_does_not_hide_heading_or_frontmatter() {
        let source = "\u{feff}# Заголовок\nтекст #тег\n";
        let found = headings(source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "Заголовок");
        assert_eq!(found[0].line, 0);
        let html = render_html(source, Path::new("/repo/index.md"), None, &[], &href, "");
        assert!(html.contains("<h1>Заголовок</h1>"));
        let source = "\u{feff}---\ntags: [проект]\n---\n# Тема\n";
        let (front, body) = split_frontmatter(source);
        assert_eq!(front, Some("---\ntags: [проект]\n---\n"));
        assert_eq!(body, "# Тема\n");
        assert_eq!(extract_tags(source), vec!["проект".to_string()]);
        assert_eq!(headings(source)[0].line, 3);
        let html = render_html(source, Path::new("/repo/index.md"), None, &[], &href, "");
        assert!(!html.contains("<hr"));
    }

    #[test]
    fn wiki_parts_follow_obsidian() {
        let parts = wiki_parts(" Заметка#Раздел | подпись ").unwrap();
        assert_eq!(parts.target, "Заметка");
        assert_eq!(parts.fragment, Some("Раздел"));
        assert_eq!(parts.alias, Some("подпись"));
        assert_eq!(parts.link, "Заметка#Раздел");
        let block = wiki_parts("Заметка#^блок").unwrap();
        assert_eq!(block.target, "Заметка");
        assert_eq!(block.fragment, Some("^блок"));
        let here = wiki_parts("#Раздел").unwrap();
        assert_eq!(here.target, "");
        assert_eq!(here.fragment, Some("Раздел"));
        let table = wiki_parts("Заметка\\|подпись").unwrap();
        assert_eq!(table.target, "Заметка");
        assert_eq!(table.alias, Some("подпись"));
        assert!(wiki_parts("").is_none());
        assert!(wiki_parts(" | подпись").is_none());
        assert!(wiki_parts("a]b").is_none());
        let line = "до [[Заметка|под]пись]] после";
        let link = wiki_at(line, line.find("[[").unwrap()).unwrap();
        assert_eq!(link.parts.alias, Some("под]пись"));
        assert_eq!(&line[link.shown..link.end], "под]пись]]");
        assert!(wiki_at("[[a[b]]", 0).is_none());
        assert!(wiki_at("[[раз\nдва]]", 0).is_none());
    }

    #[test]
    fn wiki_links_with_section_alias_and_block_point_at_the_note() {
        let source = "[[Заметка#Раздел]] [[Заметка#^блок]] [[Заметка|подпись]] [[#Раздел]] [[a[b]]";
        let links = extract_links(source);
        assert_eq!(links.len(), 3);
        assert!(links
            .iter()
            .all(|link| link.kind == LinkKind::Wiki && link.target == "Заметка"));
    }

    #[test]
    fn wiki_names_ignore_case() {
        let pin = Path::new("/repo");
        let files = vec![
            PathBuf::from("/repo/Заметка.md"),
            PathBuf::from("/repo/Папка/Итоги.MD"),
            PathBuf::from("/repo/Папка/Другое.markdown"),
        ];
        let note = vec![PathBuf::from("/repo/Заметка.md")];
        assert_eq!(resolve_wiki("заметка", pin, &files), note);
        assert_eq!(resolve_wiki("ЗАМЕТКА#Раздел|подпись", pin, &files), note);
        assert_eq!(resolve_wiki("заметка.MD", pin, &files), note);
        assert_eq!(
            resolve_wiki("папка/итоги.md", pin, &files),
            vec![PathBuf::from("/repo/Папка/Итоги.MD")]
        );
        assert_eq!(
            resolve_wiki("Другое.MARKDOWN", pin, &files),
            vec![PathBuf::from("/repo/Папка/Другое.markdown")]
        );
        assert!(resolve_wiki("#Раздел", pin, &files).is_empty());
        assert!(resolve_wiki("нет", pin, &files).is_empty());
    }

    #[test]
    fn wiki_section_opens_heading_line() {
        let dir = std::env::temp_dir().join(format!("verst_wiki_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let note = dir.join("Заметка.md");
        std::fs::write(&note, "# Начало\nтекст\n## Второй раздел\nтекст ^блок\n").unwrap();
        let from = dir.join("index.md");
        let files = vec![note.clone()];
        let html = render_html(
            "[[заметка#второй раздел]] [[Заметка#^блок|блок]]\n\n## Здесь\n[[#Здесь]]",
            &from,
            Some(&dir),
            &files,
            &href,
            "missing:",
        );
        // pulldown-cmark кодирует не-ASCII в адресе так же, как encode_path.
        let note_href = encode_path(&note.display().to_string());
        assert!(html.contains(&format!("open:{note_href}:2")));
        assert!(html.contains(&format!("open:{note_href}:3")));
        assert!(html.contains(&format!("open:{}:2", from.display())));
        assert!(html.contains("заметка &gt; второй раздел"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wiki_caption_cannot_break_the_link() {
        let pin = Path::new("/repo");
        let files = vec![PathBuf::from("/repo/a.md")];
        let html = render_html(
            "[[a|под]пись](https://example.com) *x*]]",
            Path::new("/repo/index.md"),
            Some(pin),
            &files,
            &href,
            "missing:",
        );
        assert!(html.contains(&format!(
            r#"<a href="open:{}:">под]пись](https://example.com) *x*</a>"#,
            Path::new("/repo/a.md").display()
        )));
        assert!(!html.contains(r#"href="https://example.com""#));
        let broken = render_html(
            "[[a[b]]",
            Path::new("/repo/index.md"),
            Some(pin),
            &files,
            &href,
            "missing:",
        );
        assert!(!broken.contains("<a "));
    }

    #[test]
    fn path_roundtrip() {
        let raw = "/home/alex/заметка.md";
        let back = decode_path(&encode_path(raw)).unwrap();
        assert_eq!(back, raw);
    }
}
