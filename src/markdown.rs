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

pub fn headings(source: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (line, raw) in source.lines().enumerate() {
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
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut code = false;
    while i < chars.len() {
        if chars[i] == '`' {
            code = !code;
            i += 1;
            continue;
        }
        if code {
            i += 1;
            continue;
        }
        if chars[i] == '[' && i + 1 < chars.len() && chars[i + 1] == '[' {
            if let Some(end) = find_seq(&chars, i + 2, &[']', ']']) {
                let inner: String = chars[i + 2..end].iter().collect();
                let target = wiki_target(&inner);
                if !target.is_empty() {
                    out.push(RawLink {
                        kind: LinkKind::Wiki,
                        target,
                        line: line_index,
                    });
                }
                i = end + 2;
                continue;
            }
        }
        if chars[i] == '[' && (i == 0 || chars[i - 1] != '!') {
            if let Some(close) = find_char(&chars, i + 1, ']') {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(url_end) = find_char(&chars, close + 2, ')') {
                        let target: String = chars[close + 2..url_end].iter().collect();
                        let target = target.trim().to_string();
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

fn wiki_target(inner: &str) -> String {
    inner.split('|').next().unwrap_or(inner).trim().to_string()
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

fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    chars[from..]
        .iter()
        .position(|c| *c == needle)
        .map(|i| i + from)
}

fn find_seq(chars: &[char], from: usize, seq: &[char]) -> Option<usize> {
    if seq.is_empty() || from >= chars.len() {
        return None;
    }
    chars[from..]
        .windows(seq.len())
        .position(|w| w == seq)
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
    let name = wiki_target(name);
    if name.is_empty() {
        return Vec::new();
    }
    if name.contains('/') || name.ends_with(".md") || name.ends_with(".markdown") {
        let mut relative = PathBuf::from(&name);
        if relative.extension().is_none() {
            relative.set_extension("md");
        }
        let candidate = normalize(pin.join(relative));
        return files
            .iter()
            .filter(|path| *path == &candidate)
            .cloned()
            .collect();
    }
    files
        .iter()
        .filter(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(name.as_str()))
        .cloned()
        .collect()
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
    let rewritten = rewrite_wikilinks(source, pin, files, href);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(&rewritten, options).into_offset_iter();
    let mut events = Vec::new();
    let mut seen_line = None;
    let mut in_code = false;
    for (event, range) in parser {
        let event = rewrite_link_event(event, from, href);
        let line = line_at_byte(&rewritten, range.start);
        if !in_code && seen_line != Some(line) {
            events.push(Event::Html(CowStr::from(format!(
                r#"<span id="twl{line}"></span>"#
            ))));
            seen_line = Some(line);
        }
        if matches!(event, Event::Start(Tag::CodeBlock(_))) {
            in_code = true;
        }
        let ends_code = matches!(event, Event::End(TagEnd::CodeBlock));
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

fn line_at_byte(source: &str, byte: usize) -> usize {
    let end = byte.min(source.len());
    source[..end].bytes().filter(|byte| *byte == b'\n').count()
}

fn rewrite_wikilinks(
    source: &str,
    pin: Option<&Path>,
    files: &[PathBuf],
    href: &impl Fn(LocalLink) -> String,
) -> String {
    let (front, body) = split_frontmatter(source);
    let mut out = String::new();
    if let Some(front) = front {
        out.push_str(front);
    }
    let mut in_fence = false;
    for (index, raw) in body.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push_str(raw);
            continue;
        }
        if in_fence {
            out.push_str(raw);
            continue;
        }
        out.push_str(&rewrite_line(raw, pin, files, href));
    }
    if body.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn rewrite_line(
    line: &str,
    pin: Option<&Path>,
    files: &[PathBuf],
    href: &impl Fn(LocalLink) -> String,
) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut code = false;
    while i < chars.len() {
        if chars[i] == '`' {
            code = !code;
            out.push('`');
            i += 1;
            continue;
        }
        if !code && chars[i] == '[' && i + 1 < chars.len() && chars[i + 1] == '[' {
            if let Some(end) = find_seq(&chars, i + 2, &[']', ']']) {
                let inner: String = chars[i + 2..end].iter().collect();
                out.push_str(&wiki_markdown(&inner, pin, files, href));
                i = end + 2;
                continue;
            }
        }
        if code {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        if chars[i] == '[' && (i == 0 || chars[i - 1] != '!') {
            out.push('[');
            i += 1;
            if let Some(end) = skip_link_label_and_dest(&chars, i) {
                out.extend(chars[i..end].iter());
                i = end;
            }
            continue;
        }
        if let Some(tag) = tag_at(&chars, i) {
            let label: String = chars[i..tag.end].iter().collect();
            out.push_str(&format!(
                "[{label}]({})",
                href(LocalLink::Tag { name: tag.name })
            ));
            i = tag.end;
            continue;
        }
        out.push(chars[i]);
        i += 1;
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
    let mut in_fence = false;
    for raw in body.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if let Some(tag) = tag_at(&chars, i) {
                push_tag(&mut tags, tag.name);
                i = tag.end;
                continue;
            }
            i += 1;
        }
    }
    tags
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

fn tag_at(chars: &[char], index: usize) -> Option<FoundTag> {
    if chars.get(index) != Some(&'#') {
        return None;
    }
    if index > 0 && tag_glue(chars[index - 1]) {
        return None;
    }
    if in_inline_code(chars, index) {
        return None;
    }
    let mut end = index + 1;
    let body_start = end;
    while end < chars.len() && is_tag_char(chars[end]) {
        end += 1;
    }
    if end == body_start {
        return None;
    }
    let body: String = chars[body_start..end].iter().collect();
    let name = canonical_tag(&body)?;
    Some(FoundTag { name, end })
}

fn in_inline_code(chars: &[char], index: usize) -> bool {
    chars[..index].iter().filter(|ch| **ch == '`').count() % 2 == 1
}

fn tag_glue(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '#' | '/' | '.' | ':' | '?' | '&' | '=' | '-')
}

fn is_tag_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-' | '/')
}

fn tag_body_ok(body: &str) -> bool {
    if body.is_empty() || body.chars().count() > 80 {
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

fn skip_link_label_and_dest(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    let mut depth = 1;
    while i < chars.len() {
        match chars[i] {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    if i + 1 < chars.len() && chars[i + 1] == '(' {
                        i += 2;
                        while i < chars.len() && chars[i] != ')' {
                            i += 1;
                        }
                        if i < chars.len() {
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

fn wiki_markdown(
    inner: &str,
    pin: Option<&Path>,
    files: &[PathBuf],
    href: &impl Fn(LocalLink) -> String,
) -> String {
    let target = wiki_target(inner);
    let label = inner
        .split('|')
        .nth(1)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(target.as_str());
    let link = match pin {
        Some(pin) => {
            let matches = resolve_wiki(&target, pin, files);
            match matches.as_slice() {
                [only] => LocalLink::Open {
                    path: only.clone(),
                    line: None,
                },
                [] => LocalLink::Missing {
                    label: target.clone(),
                },
                _ => LocalLink::Pick {
                    name: target.clone(),
                },
            }
        }
        None => LocalLink::Missing {
            label: target.clone(),
        },
    };
    format!("[{label}]({})", href(link))
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
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    if ext != "md" && ext != "markdown" {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > 1_048_576 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let want = slug(fragment);
    headings(&text)
        .into_iter()
        .find(|heading| {
            heading.text.to_lowercase() == fragment.to_lowercase() || slug(&heading.text) == want
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
    fn path_roundtrip() {
        let raw = "/home/alex/заметка.md";
        let back = decode_path(&encode_path(raw)).unwrap();
        assert_eq!(back, raw);
    }
}
