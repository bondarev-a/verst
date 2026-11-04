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
    Heading { level: u8 },
    /// Знаки `#` и пробел у заголовка.
    HeadingMark,
    Strong,
    Emphasis,
    Code,
    CodeBlock,
    /// Обрамление жирного/курсива/кода (`**`, `*`, `` ` ``).
    Delimiter,
    Image { dest: String },
    Link { dest: String },
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
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
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
            Event::Start(Tag::CodeBlock(_)) => {
                stack.push(Frame::CodeBlock { full: range });
            }
            Event::End(TagEnd::CodeBlock) => {
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
    out
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
}
