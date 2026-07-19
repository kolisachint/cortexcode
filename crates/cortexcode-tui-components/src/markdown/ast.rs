//! A small block/inline AST built from `pulldown-cmark`'s flat event
//! stream, standing in for `marked`'s token tree from `markdown.ts`.
//!
//! This is a reduced-fidelity substitute (see the `markdown` module docs):
//! footnotes, definition lists, math, and images are not represented.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone)]
pub enum Inline {
    Text(String),
    Code(String),
    Strong(Vec<Inline>),
    Emphasis(Vec<Inline>),
    Strikethrough(Vec<Inline>),
    Link { text: Vec<Inline>, href: String },
    Html(String),
    SoftBreak,
    HardBreak,
}

#[derive(Debug, Clone)]
#[allow(clippy::enum_variant_names)] // CodeBlock/BlockQuote are the standard markdown terms.
pub enum Block {
    Heading {
        level: u8,
        inlines: Vec<Inline>,
    },
    Paragraph(Vec<Inline>),
    CodeBlock {
        lang: Option<String>,
        text: String,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
    BlockQuote(Vec<Block>),
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
    Html(String),
}

pub fn parse_markdown(text: &str) -> Vec<Block> {
    let events: Vec<Event> =
        Parser::new_ext(text, Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES).collect();
    let mut idx = 0usize;
    parse_blocks(&events, &mut idx, None)
}

fn heading_level_to_u8(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Parse a sequence of block-level events until `stop_at` (the enclosing
/// container's end tag) is reached, or the event list is exhausted.
fn parse_blocks(events: &[Event], idx: &mut usize, stop_at: Option<TagEnd>) -> Vec<Block> {
    let mut blocks = Vec::new();
    while *idx < events.len() {
        match &events[*idx] {
            Event::End(end) if Some(*end) == stop_at => {
                *idx += 1;
                return blocks;
            }
            Event::Start(Tag::Paragraph) => {
                *idx += 1;
                let inlines = parse_inlines(events, idx, TagEnd::Paragraph);
                blocks.push(Block::Paragraph(inlines));
            }
            Event::Start(Tag::Heading { level, .. }) => {
                let level = heading_level_to_u8(*level);
                *idx += 1;
                let inlines =
                    parse_inlines(events, idx, TagEnd::Heading(heading_level_from_u8(level)));
                blocks.push(Block::Heading { level, inlines });
            }
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    pulldown_cmark::CodeBlockKind::Fenced(lang) if !lang.is_empty() => {
                        Some(lang.to_string())
                    }
                    _ => None,
                };
                *idx += 1;
                let mut text = String::new();
                while *idx < events.len() {
                    match &events[*idx] {
                        Event::Text(t) => {
                            text.push_str(t);
                            *idx += 1;
                        }
                        Event::End(TagEnd::CodeBlock) => {
                            *idx += 1;
                            break;
                        }
                        _ => *idx += 1,
                    }
                }
                let text = text.strip_suffix('\n').unwrap_or(&text).to_string();
                blocks.push(Block::CodeBlock { lang, text });
            }
            Event::Start(Tag::BlockQuote(_)) => {
                *idx += 1;
                let inner = parse_blocks(events, idx, Some(TagEnd::BlockQuote(None)));
                blocks.push(Block::BlockQuote(inner));
            }
            Event::Start(Tag::List(start)) => {
                let ordered = start.is_some();
                let start_num = start.unwrap_or(1);
                *idx += 1;
                let mut items = Vec::new();
                loop {
                    match events.get(*idx) {
                        Some(Event::Start(Tag::Item)) => {
                            *idx += 1;
                            let item_blocks = parse_blocks(events, idx, Some(TagEnd::Item));
                            items.push(item_blocks);
                        }
                        Some(Event::End(TagEnd::List(_))) => {
                            *idx += 1;
                            break;
                        }
                        None => break,
                        _ => *idx += 1,
                    }
                }
                blocks.push(Block::List {
                    ordered,
                    start: start_num,
                    items,
                });
            }
            Event::Start(Tag::Table(_aligns)) => {
                *idx += 1;
                let mut header = Vec::new();
                let mut rows = Vec::new();
                loop {
                    match events.get(*idx) {
                        Some(Event::Start(Tag::TableHead)) => {
                            *idx += 1;
                            header = parse_table_row(events, idx, TagEnd::TableHead);
                        }
                        Some(Event::Start(Tag::TableRow)) => {
                            *idx += 1;
                            rows.push(parse_table_row(events, idx, TagEnd::TableRow));
                        }
                        Some(Event::End(TagEnd::Table)) => {
                            *idx += 1;
                            break;
                        }
                        None => break,
                        _ => *idx += 1,
                    }
                }
                blocks.push(Block::Table { header, rows });
            }
            Event::Rule => {
                *idx += 1;
                blocks.push(Block::Rule);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                blocks.push(Block::Html(html.to_string()));
                *idx += 1;
            }
            Event::Start(Tag::HtmlBlock) => {
                *idx += 1;
                // Skip nested content until HtmlBlock end, collecting text.
                let mut html = String::new();
                while *idx < events.len() {
                    match &events[*idx] {
                        Event::Html(t) => {
                            html.push_str(t);
                            *idx += 1;
                        }
                        Event::End(TagEnd::HtmlBlock) => {
                            *idx += 1;
                            break;
                        }
                        _ => *idx += 1,
                    }
                }
                blocks.push(Block::Html(html));
            }
            // Tight list items (and similar contexts) hold bare inline
            // content with no enclosing Paragraph tag. Collect a run of it
            // into a synthetic paragraph.
            Event::Text(_)
            | Event::Code(_)
            | Event::SoftBreak
            | Event::HardBreak
            | Event::Start(Tag::Strong)
            | Event::Start(Tag::Emphasis)
            | Event::Start(Tag::Strikethrough)
            | Event::Start(Tag::Link { .. })
            | Event::Start(Tag::Image { .. }) => {
                let inlines = parse_inline_run(events, idx, stop_at);
                blocks.push(Block::Paragraph(inlines));
            }
            // Anything else at block level (e.g. footnote defs, metadata) is skipped.
            _ => {
                *idx += 1;
            }
        }
    }
    blocks
}

/// Like [`parse_inlines`] but for bare inline content at block level (no
/// enclosing `Paragraph`/etc. start tag): stops without consuming when it
/// reaches `block_stop_at` or a genuine block-level start tag.
fn parse_inline_run(
    events: &[Event],
    idx: &mut usize,
    block_stop_at: Option<TagEnd>,
) -> Vec<Inline> {
    let mut inlines = Vec::new();
    while *idx < events.len() {
        match &events[*idx] {
            Event::End(end) if Some(*end) == block_stop_at => return inlines,
            Event::Start(
                Tag::Paragraph
                | Tag::Heading { .. }
                | Tag::CodeBlock(_)
                | Tag::BlockQuote(_)
                | Tag::List(_)
                | Tag::Table(_)
                | Tag::HtmlBlock
                | Tag::Item,
            ) => return inlines,
            Event::Text(t) => {
                inlines.push(Inline::Text(t.to_string()));
                *idx += 1;
            }
            Event::Code(t) => {
                inlines.push(Inline::Code(t.to_string()));
                *idx += 1;
            }
            Event::SoftBreak => {
                inlines.push(Inline::SoftBreak);
                *idx += 1;
            }
            Event::HardBreak => {
                inlines.push(Inline::HardBreak);
                *idx += 1;
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                inlines.push(Inline::Html(t.to_string()));
                *idx += 1;
            }
            Event::Start(Tag::Strong) => {
                *idx += 1;
                inlines.push(Inline::Strong(parse_inlines(events, idx, TagEnd::Strong)));
            }
            Event::Start(Tag::Emphasis) => {
                *idx += 1;
                inlines.push(Inline::Emphasis(parse_inlines(
                    events,
                    idx,
                    TagEnd::Emphasis,
                )));
            }
            Event::Start(Tag::Strikethrough) => {
                *idx += 1;
                inlines.push(Inline::Strikethrough(parse_inlines(
                    events,
                    idx,
                    TagEnd::Strikethrough,
                )));
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let href = dest_url.to_string();
                *idx += 1;
                inlines.push(Inline::Link {
                    text: parse_inlines(events, idx, TagEnd::Link),
                    href,
                });
            }
            Event::Start(Tag::Image { .. }) => {
                *idx += 1;
                inlines.extend(parse_inlines(events, idx, TagEnd::Image));
            }
            _ => return inlines,
        }
    }
    inlines
}

fn heading_level_from_u8(level: u8) -> HeadingLevel {
    match level {
        1 => HeadingLevel::H1,
        2 => HeadingLevel::H2,
        3 => HeadingLevel::H3,
        4 => HeadingLevel::H4,
        5 => HeadingLevel::H5,
        _ => HeadingLevel::H6,
    }
}

fn parse_table_row(events: &[Event], idx: &mut usize, stop_at: TagEnd) -> Vec<Vec<Inline>> {
    let mut cells = Vec::new();
    loop {
        match events.get(*idx) {
            Some(Event::Start(Tag::TableCell)) => {
                *idx += 1;
                let inlines = parse_inlines(events, idx, TagEnd::TableCell);
                cells.push(inlines);
            }
            Some(Event::End(end)) if *end == stop_at => {
                *idx += 1;
                break;
            }
            None => break,
            _ => *idx += 1,
        }
    }
    cells
}

fn parse_inlines(events: &[Event], idx: &mut usize, stop_at: TagEnd) -> Vec<Inline> {
    let mut inlines = Vec::new();
    while *idx < events.len() {
        match &events[*idx] {
            Event::End(end) if *end == stop_at => {
                *idx += 1;
                return inlines;
            }
            Event::Text(t) => {
                inlines.push(Inline::Text(t.to_string()));
                *idx += 1;
            }
            Event::Code(t) => {
                inlines.push(Inline::Code(t.to_string()));
                *idx += 1;
            }
            Event::SoftBreak => {
                inlines.push(Inline::SoftBreak);
                *idx += 1;
            }
            Event::HardBreak => {
                inlines.push(Inline::HardBreak);
                *idx += 1;
            }
            Event::Html(t) | Event::InlineHtml(t) => {
                inlines.push(Inline::Html(t.to_string()));
                *idx += 1;
            }
            Event::Start(Tag::Strong) => {
                *idx += 1;
                let inner = parse_inlines(events, idx, TagEnd::Strong);
                inlines.push(Inline::Strong(inner));
            }
            Event::Start(Tag::Emphasis) => {
                *idx += 1;
                let inner = parse_inlines(events, idx, TagEnd::Emphasis);
                inlines.push(Inline::Emphasis(inner));
            }
            Event::Start(Tag::Strikethrough) => {
                *idx += 1;
                let inner = parse_inlines(events, idx, TagEnd::Strikethrough);
                inlines.push(Inline::Strikethrough(inner));
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                let href = dest_url.to_string();
                *idx += 1;
                let inner = parse_inlines(events, idx, TagEnd::Link);
                inlines.push(Inline::Link { text: inner, href });
            }
            Event::Start(Tag::Image { .. }) => {
                // Images render as their alt text (no inline image protocol here).
                *idx += 1;
                let inner = parse_inlines(events, idx, TagEnd::Image);
                inlines.extend(inner);
            }
            // Any nested block-ish content inside inline context is skipped.
            _ => {
                *idx += 1;
            }
        }
    }
    inlines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_heading_and_paragraph() {
        let blocks = parse_markdown("# Title\n\nHello *world*");
        assert_eq!(blocks.len(), 2);
        match &blocks[0] {
            Block::Heading { level, .. } => assert_eq!(*level, 1),
            _ => panic!("expected heading"),
        }
        match &blocks[1] {
            Block::Paragraph(inlines) => assert_eq!(inlines.len(), 2),
            _ => panic!("expected paragraph"),
        }
    }

    #[test]
    fn parses_code_block_with_language() {
        let blocks = parse_markdown("```rust\nfn main() {}\n```");
        match &blocks[0] {
            Block::CodeBlock { lang, text } => {
                assert_eq!(lang.as_deref(), Some("rust"));
                assert_eq!(text, "fn main() {}");
            }
            _ => panic!("expected code block"),
        }
    }

    #[test]
    fn parses_list_items() {
        let blocks = parse_markdown("- a\n- b\n");
        match &blocks[0] {
            Block::List { ordered, items, .. } => {
                assert!(!ordered);
                assert_eq!(items.len(), 2);
            }
            _ => panic!("expected list"),
        }
    }

    #[test]
    fn parses_table() {
        let blocks = parse_markdown("| a | b |\n|---|---|\n| 1 | 2 |\n");
        match &blocks[0] {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 1);
            }
            _ => panic!("expected table"),
        }
    }

    #[test]
    fn parses_blockquote() {
        let blocks = parse_markdown("> quoted text\n");
        match &blocks[0] {
            Block::BlockQuote(inner) => assert_eq!(inner.len(), 1),
            _ => panic!("expected blockquote"),
        }
    }
}
