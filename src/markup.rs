use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

/// Участок Markdown для меток режима рендеринга.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkSpan {
    pub kind: MarkKind,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkKind {
    Heading {
        level: u8,
    },
    /// Знаки `#` и пробел у заголовка.
    HeadingMark,
    Strong,
    Emphasis,
    Code,
    CodeBlock,
    /// Обрамление жирного/курсива/кода (`**`, `*`, `` ` ``).
    Delimiter,
    Image {
        dest: String,
    },
    Link {
        dest: String,
    },
    /// Вики-ссылка `[[цель]]` или `[[цель|подпись]]`. Видна подпись или цель.
    /// `target` — всё до `|`, вместе с `#раздел`; части даёт `markdown::wiki_parts`.
    WikiLink {
        target: String,
    },
    /// Обрамление ссылки или картинки, скрываемое вне строки курсора.
    LinkMarkup,
}

#[derive(Clone)]
enum Frame {
    Heading {
        level: u8,
        full: std::ops::Range<usize>,
        texts: Vec<std::ops::Range<usize>>,
    },
    Strong {
        full: std::ops::Range<usize>,
        texts: Vec<std::ops::Range<usize>>,
    },
    Emphasis {
        full: std::ops::Range<usize>,
        texts: Vec<std::ops::Range<usize>>,
    },
    Image {
        dest: String,
        full: std::ops::Range<usize>,
    },
    Link {
        dest: String,
        full: std::ops::Range<usize>,
        texts: Vec<std::ops::Range<usize>>,
    },
    CodeBlock {
        full: std::ops::Range<usize>,
    },
}

/// Смещения по исходному тексту, без переписывания вики-ссылок.
pub fn mark_spans(source: &str) -> Vec<MarkSpan> {
    // Метка порядка байтов остаётся в буфере редактора, но мешает разбору
    // первой строки. Разбираем текст без неё и сдвигаем смещения обратно.
    let text = crate::markdown::strip_bom(source);
    let bom = source.len() - text.len();
    let mut out = spans_of(text);
    if bom > 0 {
        for span in &mut out {
            span.start += bom;
            span.end += bom;
        }
    }
    out
}

fn spans_of(source: &str) -> Vec<MarkSpan> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    // Шапка YAML иначе читается как линия и заголовок второго уровня.
    options.insert(Options::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    let parser = Parser::new_ext(source, options).into_offset_iter();

    let mut out = Vec::new();
    let mut stack: Vec<Frame> = Vec::new();

    for (event, range) in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                stack.push(Frame::Heading {
                    level: heading_level(level),
                    full: range,
                    texts: Vec::new(),
                });
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(Frame::Heading { level, full, texts }) = pop_heading(&mut stack) {
                    out.push(MarkSpan {
                        kind: MarkKind::Heading { level },
                        start: full.start,
                        end: full.end,
                    });
                    if let Some(first) = texts.first() {
                        if first.start > full.start {
                            out.push(MarkSpan {
                                kind: MarkKind::HeadingMark,
                                start: full.start,
                                end: first.start,
                            });
                        }
                    }
                }
            }
            Event::Start(Tag::Strong) => {
                stack.push(Frame::Strong {
                    full: range,
                    texts: Vec::new(),
                });
            }
            Event::End(TagEnd::Strong) => {
                if let Some(Frame::Strong { full, texts }) = pop_strong(&mut stack) {
                    push_wrapped(&mut out, MarkKind::Strong, &full, &texts);
                }
            }
            Event::Start(Tag::Emphasis) => {
                stack.push(Frame::Emphasis {
                    full: range,
                    texts: Vec::new(),
                });
            }
            Event::End(TagEnd::Emphasis) => {
                if let Some(Frame::Emphasis { full, texts }) = pop_em(&mut stack) {
                    push_wrapped(&mut out, MarkKind::Emphasis, &full, &texts);
                }
            }
            Event::Code(_) => {
                let inner_start = range.start + 1;
                let inner_end = range.end.saturating_sub(1);
                if inner_start < inner_end && inner_end <= source.len() {
                    out.push(MarkSpan {
                        kind: MarkKind::Delimiter,
                        start: range.start,
                        end: inner_start,
                    });
                    out.push(MarkSpan {
                        kind: MarkKind::Code,
                        start: inner_start,
                        end: inner_end,
                    });
                    out.push(MarkSpan {
                        kind: MarkKind::Delimiter,
                        start: inner_end,
                        end: range.end,
                    });
                } else {
                    out.push(MarkSpan {
                        kind: MarkKind::Code,
                        start: range.start,
                        end: range.end,
                    });
                }
            }
            // Шапка показывается как блок кода: моноширинно, без меток заголовка.
            Event::Start(Tag::CodeBlock(_)) | Event::Start(Tag::MetadataBlock(_)) => {
                stack.push(Frame::CodeBlock { full: range });
            }
            Event::End(TagEnd::CodeBlock) | Event::End(TagEnd::MetadataBlock(_)) => {
                if let Some(Frame::CodeBlock { full }) = pop_code_block(&mut stack) {
                    out.push(MarkSpan {
                        kind: MarkKind::CodeBlock,
                        start: full.start,
                        end: full.end.max(range.end),
                    });
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                stack.push(Frame::Image {
                    dest: dest_url.to_string(),
                    full: range,
                });
            }
            Event::End(TagEnd::Image) => {
                if let Some(Frame::Image { dest, full }) = pop_image(&mut stack) {
                    let end = full.end.max(range.end);
                    out.push(MarkSpan {
                        kind: MarkKind::Image { dest },
                        start: full.start,
                        end,
                    });
                    out.push(MarkSpan {
                        kind: MarkKind::LinkMarkup,
                        start: full.start,
                        end,
                    });
                }
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                stack.push(Frame::Link {
                    dest: dest_url.to_string(),
                    full: range,
                    texts: Vec::new(),
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(Frame::Link { dest, full, texts }) = pop_link(&mut stack) {
                    let end = full.end.max(range.end);
                    if let Some(first) = texts.first() {
                        if first.start > full.start {
                            out.push(MarkSpan {
                                kind: MarkKind::LinkMarkup,
                                start: full.start,
                                end: first.start,
                            });
                        }
                        if let Some(last) = texts.last() {
                            if last.end < end {
                                out.push(MarkSpan {
                                    kind: MarkKind::LinkMarkup,
                                    start: last.end,
                                    end,
                                });
                            }
                        }
                    }
                    out.push(MarkSpan {
                        kind: MarkKind::Link { dest },
                        start: full.start,
                        end,
                    });
                }
            }
            Event::Text(_) | Event::SoftBreak | Event::HardBreak => {
                push_text(&mut stack, range);
            }
            _ => {}
        }
    }
    let code: Vec<(usize, usize)> = out
        .iter()
        .filter(|span| matches!(span.kind, MarkKind::Code | MarkKind::CodeBlock))
        .map(|span| (span.start, span.end))
        .collect();
    wiki_spans(source, &code, &mut out);
    out
}

/// pulldown-cmark не знает `[[…]]`, поэтому вики-ссылки ищутся отдельно,
/// вне кода, тем же разбором, что и в предпросмотре и индексе.
fn wiki_spans(source: &str, code: &[(usize, usize)], out: &mut Vec<MarkSpan>) {
    let mut at = 0;
    while let Some(found) = source[at..].find("[[") {
        let start = at + found;
        let Some(link) = crate::markdown::wiki_at(source, start) else {
            at = start + 1;
            continue;
        };
        at = link.end;
        if code.iter().any(|(a, b)| start < *b && link.end > *a) {
            continue;
        }
        out.push(MarkSpan {
            kind: MarkKind::LinkMarkup,
            start,
            end: link.shown,
        });
        out.push(MarkSpan {
            kind: MarkKind::LinkMarkup,
            start: link.end - 2,
            end: link.end,
        });
        out.push(MarkSpan {
            kind: MarkKind::WikiLink {
                target: link.parts.link.to_string(),
            },
            start,
            end: link.end,
        });
    }
}

fn push_text(stack: &mut [Frame], range: std::ops::Range<usize>) {
    match stack.last_mut() {
        Some(Frame::Heading { texts, .. })
        | Some(Frame::Strong { texts, .. })
        | Some(Frame::Emphasis { texts, .. })
        | Some(Frame::Link { texts, .. }) => texts.push(range),
        _ => {}
    }
}

fn pop_heading(stack: &mut Vec<Frame>) -> Option<Frame> {
    match stack.pop() {
        Some(frame @ Frame::Heading { .. }) => Some(frame),
        Some(other) => {
            stack.push(other);
            None
        }
        None => None,
    }
}

fn pop_strong(stack: &mut Vec<Frame>) -> Option<Frame> {
    match stack.pop() {
        Some(frame @ Frame::Strong { .. }) => Some(frame),
        Some(other) => {
            stack.push(other);
            None
        }
        None => None,
    }
}

fn pop_em(stack: &mut Vec<Frame>) -> Option<Frame> {
    match stack.pop() {
        Some(frame @ Frame::Emphasis { .. }) => Some(frame),
        Some(other) => {
            stack.push(other);
            None
        }
        None => None,
    }
}

fn pop_code_block(stack: &mut Vec<Frame>) -> Option<Frame> {
    match stack.pop() {
        Some(frame @ Frame::CodeBlock { .. }) => Some(frame),
        Some(other) => {
            stack.push(other);
            None
        }
        None => None,
    }
}

fn pop_image(stack: &mut Vec<Frame>) -> Option<Frame> {
    match stack.pop() {
        Some(frame @ Frame::Image { .. }) => Some(frame),
        Some(other) => {
            stack.push(other);
            None
        }
        None => None,
    }
}

fn pop_link(stack: &mut Vec<Frame>) -> Option<Frame> {
    match stack.pop() {
        Some(frame @ Frame::Link { .. }) => Some(frame),
        Some(other) => {
            stack.push(other);
            None
        }
        None => None,
    }
}

fn push_wrapped(
    out: &mut Vec<MarkSpan>,
    kind: MarkKind,
    full: &std::ops::Range<usize>,
    texts: &[std::ops::Range<usize>],
) {
    if let (Some(first), Some(last)) = (texts.first(), texts.last()) {
        if first.start > full.start {
            out.push(MarkSpan {
                kind: MarkKind::Delimiter,
                start: full.start,
                end: first.start,
            });
        }
        out.push(MarkSpan {
            kind,
            start: first.start,
            end: last.end,
        });
        if last.end < full.end {
            out.push(MarkSpan {
                kind: MarkKind::Delimiter,
                start: last.end,
                end: full.end,
            });
        }
    } else {
        out.push(MarkSpan {
            kind,
            start: full.start,
            end: full.end,
        });
    }
}

fn heading_level(level: pulldown_cmark::HeadingLevel) -> u8 {
    match level {
        pulldown_cmark::HeadingLevel::H1 => 1,
        pulldown_cmark::HeadingLevel::H2 => 2,
        pulldown_cmark::HeadingLevel::H3 => 3,
        pulldown_cmark::HeadingLevel::H4 => 4,
        pulldown_cmark::HeadingLevel::H5 => 5,
        pulldown_cmark::HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_heading_strong_and_image() {
        let source = "# Title\n\nHello **bold**\n\n![a](./x.png)\n";
        let spans = mark_spans(source);
        assert!(spans
            .iter()
            .any(|s| matches!(s.kind, MarkKind::Heading { level: 1 })));
        assert!(spans.iter().any(|s| s.kind == MarkKind::HeadingMark));
        assert!(spans.iter().any(|s| s.kind == MarkKind::Strong));
        assert!(spans.iter().any(|s| s.kind == MarkKind::Delimiter));
        assert!(spans
            .iter()
            .any(|s| matches!(&s.kind, MarkKind::Image { dest } if dest == "./x.png")));
    }

    fn markup_of(source: &str, spans: &[MarkSpan]) -> Vec<String> {
        spans
            .iter()
            .filter(|s| s.kind == MarkKind::LinkMarkup)
            .map(|s| source[s.start..s.end].to_string())
            .collect()
    }

    #[test]
    fn wiki_link_hides_brackets() {
        let source = "См. [[Заметка]] тут.\n";
        let spans = mark_spans(source);
        assert_eq!(markup_of(source, &spans), vec!["[[", "]]"]);
        let link = spans
            .iter()
            .find(|s| matches!(s.kind, MarkKind::WikiLink { .. }))
            .unwrap();
        assert_eq!(&source[link.start..link.end], "[[Заметка]]");
        assert_eq!(
            link.kind,
            MarkKind::WikiLink {
                target: "Заметка".to_string()
            }
        );
    }

    #[test]
    fn wiki_link_with_alias_shows_alias() {
        let source = "[[папка/цель|подпись]]\n";
        let spans = mark_spans(source);
        assert_eq!(markup_of(source, &spans), vec!["[[папка/цель|", "]]"]);
        assert!(spans.iter().any(
            |s| matches!(&s.kind, MarkKind::WikiLink { target } if target == "папка/цель")
        ));
    }

    #[test]
    fn wiki_link_with_section_keeps_it_in_target() {
        let source = "[[Заметка#Раздел|подпись]] и [[#Здесь]]\n";
        let spans = mark_spans(source);
        assert_eq!(
            markup_of(source, &spans),
            vec!["[[Заметка#Раздел|", "]]", "[[", "]]"]
        );
        let targets: Vec<&str> = spans
            .iter()
            .filter_map(|s| match &s.kind {
                MarkKind::WikiLink { target } => Some(target.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(targets, vec!["Заметка#Раздел", "#Здесь"]);
    }

    #[test]
    fn wiki_link_inside_code_is_text() {
        let source = "`[[код]]`\n\n```\n[[блок]]\n```\n";
        let spans = mark_spans(source);
        assert!(!spans
            .iter()
            .any(|s| matches!(s.kind, MarkKind::WikiLink { .. })));
    }

    #[test]
    fn frontmatter_is_a_code_block_not_a_heading() {
        let source = "---\ntags: [проект]\n---\n# Заголовок\n";
        let spans = mark_spans(source);
        let headings: Vec<&MarkSpan> = spans
            .iter()
            .filter(|s| matches!(s.kind, MarkKind::Heading { .. }))
            .collect();
        assert_eq!(headings.len(), 1);
        assert_eq!(&source[headings[0].start..headings[0].end], "# Заголовок\n");
        let block = spans
            .iter()
            .find(|s| s.kind == MarkKind::CodeBlock)
            .unwrap();
        assert_eq!(block.start, 0);
        assert!(source[block.start..block.end].contains("tags: [проект]"));
    }

    #[test]
    fn bom_keeps_heading_and_offsets() {
        let source = "\u{feff}# Заголовок\n[[Заметка]]\n";
        let spans = mark_spans(source);
        let heading = spans
            .iter()
            .find(|s| matches!(s.kind, MarkKind::Heading { level: 1 }))
            .unwrap();
        assert_eq!(&source[heading.start..heading.end], "# Заголовок\n");
        let mark = spans.iter().find(|s| s.kind == MarkKind::HeadingMark).unwrap();
        assert_eq!(&source[mark.start..mark.end], "# ");
        let link = spans
            .iter()
            .find(|s| matches!(s.kind, MarkKind::WikiLink { .. }))
            .unwrap();
        assert_eq!(&source[link.start..link.end], "[[Заметка]]");
    }

    #[test]
    fn broken_wiki_links_are_skipped() {
        let source = "[[]] и [[раз\nдва]] и [[a[b]]\n";
        let spans = mark_spans(source);
        assert!(!spans
            .iter()
            .any(|s| matches!(s.kind, MarkKind::WikiLink { .. })));
    }
}
