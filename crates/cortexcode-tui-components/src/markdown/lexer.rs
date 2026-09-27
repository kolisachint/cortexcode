//! A port of the pinned `marked` (v15) lexer and tokenizer, GFM flavour
//! (`gfm: true`, `breaks: false`, `pedantic: false`), with markdown.ts's
//! strict-strikethrough `del` override.
//!
//! The port is literal: the rules are `marked`'s own regex sources
//! (`rules_gen.rs`, run through [`super::js_regex`]) and the tokenizer
//! methods follow `Tokenizer.ts`/`Lexer.ts` step by step, including the
//! deferred inline queue and its quirks. Differences are confined to string
//! indexing: offsets are bytes, so the masked copy of the source the
//! emphasis tokenizer scans is kept byte-aligned with the source (masks are
//! as many bytes long as what they cover, where `marked` keeps UTF-16
//! lengths). The two only disagree for an escaped astral-plane symbol
//! (`\😀`), where `marked`'s own mask misaligns.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use super::js_regex::{js_slice_from, js_trim, js_trim_end, JsRegex};
use super::rules::{list_item_regex, IndentRules, RULES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    Space,
    Code,
    Heading,
    Hr,
    Blockquote,
    List,
    Html,
    Table,
    Paragraph,
    Text,
    Escape,
    Link,
    Image,
    Strong,
    Em,
    Codespan,
    Br,
    Del,
}

impl TokenType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Space => "space",
            Self::Code => "code",
            Self::Heading => "heading",
            Self::Hr => "hr",
            Self::Blockquote => "blockquote",
            Self::List => "list",
            Self::Html => "html",
            Self::Table => "table",
            Self::Paragraph => "paragraph",
            Self::Text => "text",
            Self::Escape => "escape",
            Self::Link => "link",
            Self::Image => "image",
            Self::Strong => "strong",
            Self::Em => "em",
            Self::Codespan => "codespan",
            Self::Br => "br",
            Self::Del => "del",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableCell {
    pub text: String,
    pub tokens: Vec<Token>,
    pub header: bool,
    pub align: Option<Align>,
    slot: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableData {
    pub header: Vec<TableCell>,
    pub align: Vec<Option<Align>>,
    pub rows: Vec<Vec<TableCell>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub raw: String,
    pub task: bool,
    pub checked: Option<bool>,
    pub loose: bool,
    pub text: String,
    pub tokens: Vec<Token>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListData {
    pub ordered: bool,
    /// The first item's number (`""` in `marked` for a bullet list).
    pub start: Option<u32>,
    pub loose: bool,
    pub items: Vec<ListItem>,
}

/// One `marked` token. Fields a token type does not carry stay `None`, so a
/// token maps one-to-one onto the object `marked` builds.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenType,
    pub raw: String,
    pub text: Option<String>,
    pub tokens: Option<Vec<Token>>,
    pub depth: Option<u8>,
    pub lang: Option<String>,
    /// `codeBlockStyle: "indented"`.
    pub indented: bool,
    pub href: Option<String>,
    /// `title`: absent (`None`), `null` (`Some(None)`) or a string.
    pub title: Option<Option<String>>,
    pub block: Option<bool>,
    pub pre: Option<bool>,
    pub in_link: Option<bool>,
    pub in_raw_block: Option<bool>,
    pub escaped: Option<bool>,
    pub list: Option<Box<ListData>>,
    pub table: Option<Box<TableData>>,
    /// Inline-queue slot whose result becomes `tokens` once lexing ends.
    slot: Option<usize>,
}

impl Token {
    fn new(kind: TokenType, raw: impl Into<String>) -> Self {
        Self {
            kind,
            raw: raw.into(),
            text: None,
            tokens: None,
            depth: None,
            lang: None,
            indented: false,
            href: None,
            title: None,
            block: None,
            pre: None,
            in_link: None,
            in_raw_block: None,
            escaped: None,
            list: None,
            table: None,
            slot: None,
        }
    }

    fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    fn with_tokens(mut self, tokens: Vec<Token>) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// `token.text` (`""` when the token has none).
    pub fn text(&self) -> &str {
        self.text.as_deref().unwrap_or("")
    }

    /// `token.tokens` (empty when the token has none).
    pub fn children(&self) -> &[Token] {
        self.tokens.as_deref().unwrap_or(&[])
    }
}

/// A link definition (`[tag]: href "title"`).
#[derive(Debug, Clone, PartialEq)]
pub struct LinkDef {
    pub href: String,
    pub title: Option<String>,
}

/// The result of lexing a document: its tokens and `tokens.links`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    /// Link definitions, in definition order.
    pub links: Vec<(String, LinkDef)>,
}

/// `new Marked().lexer(src)` with markdown.ts's options.
pub fn lex(src: &str) -> Lexed {
    let mut lexer = Lexer::default();
    let src = RULES.other.carriage_return.replace(src, true, "\n");
    let mut tokens = Vec::new();
    lexer.block_tokens(&src, &mut tokens, false);
    let queue = std::mem::take(&mut lexer.inline_queue);
    let mut results: Vec<Option<Vec<Token>>> = vec![None; lexer.slots];
    for (src, slot) in queue {
        results[slot] = Some(lexer.inline_tokens(&src));
    }
    resolve(&mut tokens, &mut results);
    Lexed {
        tokens,
        links: lexer.links,
    }
}

/// `Lexer.lexInline(src)`: inline tokens alone, with no link definitions.
pub fn lex_inline(src: &str) -> Vec<Token> {
    Lexer::default().inline_tokens(src)
}

fn resolve(tokens: &mut [Token], results: &mut [Option<Vec<Token>>]) {
    for token in tokens {
        if let Some(slot) = token.slot.take() {
            token.tokens = Some(results[slot].take().unwrap_or_default());
        }
        if let Some(children) = token.tokens.as_mut() {
            resolve(children, results);
        }
        if let Some(list) = token.list.as_mut() {
            for item in &mut list.items {
                resolve(&mut item.tokens, results);
            }
        }
        if let Some(table) = token.table.as_mut() {
            for cell in table
                .header
                .iter_mut()
                .chain(table.rows.iter_mut().flatten())
            {
                if let Some(slot) = cell.slot.take() {
                    cell.tokens = results[slot].take().unwrap_or_default();
                }
                resolve(&mut cell.tokens, results);
            }
        }
    }
}

#[derive(Default)]
struct State {
    in_link: bool,
    in_raw_block: bool,
    top: bool,
}

struct Lexer {
    state: State,
    links: Vec<(String, LinkDef)>,
    inline_queue: Vec<(String, usize)>,
    slots: usize,
}

impl Default for Lexer {
    fn default() -> Self {
        Self {
            state: State {
                in_link: false,
                in_raw_block: false,
                top: true,
            },
            links: Vec::new(),
            inline_queue: Vec::new(),
            slots: 0,
        }
    }
}

thread_local! {
    static INDENT_RULES: RefCell<HashMap<usize, Rc<IndentRules>>> = RefCell::new(HashMap::new());
    static ITEM_RULES: RefCell<HashMap<String, Rc<JsRegex>>> = RefCell::new(HashMap::new());
}

fn indent_rules(indent: usize) -> Rc<IndentRules> {
    INDENT_RULES.with(|m| {
        m.borrow_mut()
            .entry(indent)
            .or_insert_with(|| Rc::new(IndentRules::new(indent)))
            .clone()
    })
}

fn item_rule(bull: &str) -> Rc<JsRegex> {
    ITEM_RULES.with(|m| {
        m.borrow_mut()
            .entry(bull.to_string())
            .or_insert_with(|| Rc::new(list_item_regex(bull)))
            .clone()
    })
}

/// `rtrim(str, c, invert)` from `helpers.ts`.
fn rtrim(s: &str, c: char, invert: bool) -> &str {
    let mut end = s.len();
    for (i, ch) in s.char_indices().rev() {
        if (ch == c) != invert {
            end = i;
        } else {
            break;
        }
    }
    &s[..end]
}

/// `splitCells(tableRow, count)` from `helpers.ts`.
fn split_cells(row: &str, count: Option<usize>) -> Vec<String> {
    let bytes = row.as_bytes();
    let mut marked = String::with_capacity(row.len() + 8);
    for (i, ch) in row.char_indices() {
        if ch == '|' {
            let mut escaped = false;
            let mut j = i;
            while j > 0 && bytes[j - 1] == b'\\' {
                escaped = !escaped;
                j -= 1;
            }
            marked.push_str(if escaped { "|" } else { " |" });
        } else {
            marked.push(ch);
        }
    }
    let mut cells: Vec<String> = marked.split(" |").map(str::to_string).collect();
    if js_trim(&cells[0]).is_empty() {
        cells.remove(0);
    }
    if cells.last().is_some_and(|c| js_trim(c).is_empty()) {
        cells.pop();
    }
    if let Some(count) = count {
        if cells.len() > count {
            cells.truncate(count);
        } else {
            while cells.len() < count {
                cells.push(String::new());
            }
        }
    }
    cells
        .iter()
        .map(|c| js_trim(c).replace("\\|", "|"))
        .collect()
}

/// `findClosingBracket(str, "()")` (a byte index, -1 or -2).
fn find_closing_paren(s: &str) -> isize {
    if !s.contains(')') {
        return -1;
    }
    let bytes = s.as_bytes();
    let mut level = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 1,
            b'(' => level += 1,
            b')' => {
                level -= 1;
                if level < 0 {
                    return i as isize;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if level > 0 {
        -2
    } else {
        -1
    }
}

fn indent_code_compensation(raw: &str, text: &str) -> String {
    let Some(m) = RULES.other.indent_code_compensation.exec(raw) else {
        return text.to_string();
    };
    let indent_to_code = m.get(1).unwrap();
    let n = indent_to_code.chars().map(char::len_utf16).sum::<usize>();
    text.split('\n')
        .map(|node| match RULES.other.beginning_space.exec(node) {
            None => node.to_string(),
            Some(space) => {
                let len = space.whole().chars().map(char::len_utf16).sum::<usize>();
                if len >= n {
                    js_slice_from(node, n).to_string()
                } else {
                    node.to_string()
                }
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn unescape_punct(s: &str) -> String {
    RULES.inline.any_punctuation.replace(s, true, "$1")
}

/// Replace `hay[start..end]` with `[aaa…]` of the same byte length.
fn mask(hay: &mut String, start: usize, end: usize, fill: &str) {
    let len = end - start;
    let masked = if fill == "+" {
        "+".repeat(len)
    } else {
        format!("[{}]", "a".repeat(len - 2))
    };
    hay.replace_range(start..end, &masked);
}

/// `s.substring(0, s.length - n)` (clamped, on a character boundary).
fn cut_end(s: &str, n: usize) -> &str {
    let mut end = s.len().saturating_sub(n);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// `src.substring(n)`: clamped, and (for byte offsets that `marked` counts
/// in UTF-16) moved forward onto a character boundary.
fn advance(src: &str, n: usize) -> &str {
    if n >= src.len() {
        return "";
    }
    let mut n = n;
    while !src.is_char_boundary(n) {
        n += 1;
    }
    &src[n..]
}

fn first_line(s: &str) -> &str {
    s.split('\n').next().unwrap_or("")
}

impl Lexer {
    fn queue_inline(&mut self, src: &str) -> usize {
        let slot = self.slots;
        self.slots += 1;
        self.inline_queue.push((src.to_string(), slot));
        slot
    }

    fn has_link(&self, tag: &str) -> bool {
        self.links.iter().any(|(t, _)| t == tag)
    }

    // ---- block tokenizers ------------------------------------------------

    fn space(&self, src: &str) -> Option<Token> {
        let cap = RULES.block.newline.exec(src)?;
        if cap.whole().is_empty() {
            return None;
        }
        Some(Token::new(TokenType::Space, cap.whole()))
    }

    fn code(&self, src: &str) -> Option<Token> {
        let cap = RULES.block.code.exec(src)?;
        let text = RULES
            .other
            .code_remove_indent
            .replace(cap.whole(), true, "");
        let mut t = Token::new(TokenType::Code, cap.whole()).with_text(rtrim(&text, '\n', false));
        t.indented = true;
        Some(t)
    }

    fn fences(&self, src: &str) -> Option<Token> {
        let cap = RULES.block.fences.exec(src)?;
        let raw = cap.whole();
        let text = indent_code_compensation(raw, cap.get(3).unwrap_or(""));
        let lang = cap.get(2).unwrap_or("");
        let lang = if lang.is_empty() {
            String::new()
        } else {
            unescape_punct(js_trim(lang))
        };
        let mut t = Token::new(TokenType::Code, raw).with_text(text);
        t.lang = Some(lang);
        Some(t)
    }

    fn heading(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.heading.exec(src)?;
        let mut text = js_trim(cap.get(2).unwrap()).to_string();
        if RULES.other.ending_hash.test(&text) {
            let trimmed = rtrim(&text, '#', false);
            if trimmed.is_empty() || RULES.other.ending_space_char.test(trimmed) {
                text = js_trim(trimmed).to_string();
            }
        }
        let mut t = Token::new(TokenType::Heading, cap.whole()).with_text(text.clone());
        t.depth = Some(cap.get(1).unwrap().len() as u8);
        t.slot = Some(self.queue_inline(&text));
        Some(t)
    }

    fn hr(&self, src: &str) -> Option<Token> {
        let cap = RULES.block.hr.exec(src)?;
        Some(Token::new(TokenType::Hr, rtrim(cap.whole(), '\n', false)))
    }

    fn blockquote(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.blockquote.exec(src)?;
        let mut lines: Vec<String> = rtrim(cap.whole(), '\n', false)
            .split('\n')
            .map(str::to_string)
            .collect();
        let mut raw = String::new();
        let mut text = String::new();
        let mut tokens: Vec<Token> = Vec::new();
        while !lines.is_empty() {
            let mut in_blockquote = false;
            let mut current_lines = Vec::new();
            let mut i = 0;
            while i < lines.len() {
                if RULES.other.blockquote_start.test(&lines[i]) {
                    current_lines.push(lines[i].clone());
                    in_blockquote = true;
                } else if !in_blockquote {
                    current_lines.push(lines[i].clone());
                } else {
                    break;
                }
                i += 1;
            }
            lines.drain(..i);
            let current_raw = current_lines.join("\n");
            let current_text = RULES.other.blockquote_setext_replace2.replace(
                &RULES
                    .other
                    .blockquote_setext_replace
                    .replace(&current_raw, true, "\n    $1"),
                true,
                "",
            );
            raw = if raw.is_empty() {
                current_raw
            } else {
                format!("{raw}\n{current_raw}")
            };
            text = if text.is_empty() {
                current_text.clone()
            } else {
                format!("{text}\n{current_text}")
            };
            let top = self.state.top;
            self.state.top = true;
            self.block_tokens(&current_text, &mut tokens, true);
            self.state.top = top;
            if lines.is_empty() {
                break;
            }
            let Some(last) = tokens.last() else { continue };
            match last.kind {
                TokenType::Code => break,
                TokenType::Blockquote => {
                    let old_raw = last.raw.clone();
                    let old_text = last.text().to_string();
                    let new_text = format!("{old_raw}\n{}", lines.join("\n"));
                    let new_token = self.blockquote(&new_text).unwrap();
                    raw = format!("{}{}", cut_end(&raw, old_raw.len()), new_token.raw);
                    text = format!("{}{}", cut_end(&text, old_text.len()), new_token.text());
                    *tokens.last_mut().unwrap() = new_token;
                    break;
                }
                TokenType::List => {
                    let old_raw = last.raw.clone();
                    let new_text = format!("{old_raw}\n{}", lines.join("\n"));
                    let new_token = self.list(&new_text).unwrap();
                    raw = format!("{}{}", cut_end(&raw, old_raw.len()), new_token.raw);
                    text = format!("{}{}", cut_end(&text, old_raw.len()), new_token.raw);
                    lines = new_text[new_token.raw.len()..]
                        .split('\n')
                        .map(str::to_string)
                        .collect();
                    *tokens.last_mut().unwrap() = new_token;
                    continue;
                }
                _ => {}
            }
        }
        Some(
            Token::new(TokenType::Blockquote, raw)
                .with_tokens(tokens)
                .with_text(text),
        )
    }

    fn list(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.list.exec(src)?;
        let mut src = src.to_string();
        let bull = js_trim(cap.get(1).unwrap()).to_string();
        let ordered = bull.len() > 1;
        let mut list = ListData {
            ordered,
            start: if ordered {
                bull[..bull.len() - 1].parse().ok()
            } else {
                None
            },
            loose: false,
            items: Vec::new(),
        };
        let mut list_raw = String::new();
        let bull = if ordered {
            format!(r"\d{{1,9}}\{}", &bull[bull.len() - 1..])
        } else {
            format!(r"\{bull}")
        };
        let item_regex = item_rule(&bull);
        let mut ends_with_blank_line = false;
        while !src.is_empty() {
            let mut end_early = false;
            let Some(cap) = item_regex.exec(&src) else {
                break;
            };
            if RULES.block.hr.test(&src) {
                break;
            }
            let mut raw = cap.whole().to_string();
            let cap1 = cap.get(1).unwrap().to_string();
            let cap2 = cap.get(2).unwrap().to_string();
            src = src[raw.len()..].to_string();
            let mut line =
                RULES
                    .other
                    .list_replace_tabs
                    .replace_with(first_line(&cap2), false, |m| {
                        " ".repeat(3 * m.whole().len())
                    });
            let mut next_line = first_line(&src).to_string();
            let mut blank_line = js_trim(&line).is_empty();
            let mut item_contents;
            let indent: usize;
            if blank_line {
                indent = cap1.len() + 1;
                item_contents = String::new();
            } else {
                let found = RULES.other.non_space_char.search(&cap2);
                let found = if found > 4 { 1 } else { found.max(0) as usize };
                item_contents = js_slice_from(&line, found).to_string();
                indent = found + cap1.len();
            }
            if blank_line && RULES.other.blank_line.test(&next_line) {
                raw.push_str(&next_line);
                raw.push('\n');
                src = src.get(next_line.len() + 1..).unwrap_or("").to_string();
                end_early = true;
            }
            if !end_early {
                let rules = indent_rules(indent);
                while !src.is_empty() {
                    let raw_line = first_line(&src).to_string();
                    next_line = raw_line.clone();
                    let next_line_without_tabs = next_line.replace('\t', "    ");
                    if rules.fences_begin.test(&next_line)
                        || rules.heading_begin.test(&next_line)
                        || rules.html_begin.test(&next_line)
                        || rules.next_bullet.test(&next_line)
                        || rules.hr.test(&next_line)
                    {
                        break;
                    }
                    if RULES.other.non_space_char.search(&next_line_without_tabs) >= indent as isize
                        || js_trim(&next_line).is_empty()
                    {
                        item_contents.push('\n');
                        item_contents.push_str(js_slice_from(&next_line_without_tabs, indent));
                    } else {
                        if blank_line {
                            break;
                        }
                        if RULES
                            .other
                            .non_space_char
                            .search(&line.replace('\t', "    "))
                            >= 4
                        {
                            break;
                        }
                        if rules.fences_begin.test(&line)
                            || rules.heading_begin.test(&line)
                            || rules.hr.test(&line)
                        {
                            break;
                        }
                        item_contents.push('\n');
                        item_contents.push_str(&next_line);
                    }
                    if !blank_line && js_trim(&next_line).is_empty() {
                        blank_line = true;
                    }
                    raw.push_str(&raw_line);
                    raw.push('\n');
                    src = src.get(raw_line.len() + 1..).unwrap_or("").to_string();
                    line = js_slice_from(&next_line_without_tabs, indent).to_string();
                }
            }
            if !list.loose {
                if ends_with_blank_line {
                    list.loose = true;
                } else if RULES.other.double_blank_line.test(&raw) {
                    ends_with_blank_line = true;
                }
            }
            let mut task = false;
            let mut checked = None;
            if let Some(m) = RULES.other.list_is_task.exec(&item_contents) {
                task = true;
                checked = Some(m.whole() != "[ ] ");
                item_contents = RULES
                    .other
                    .list_replace_task
                    .replace(&item_contents, false, "");
            }
            list_raw.push_str(&raw);
            list.items.push(ListItem {
                raw,
                task,
                checked,
                loose: false,
                text: item_contents,
                tokens: Vec::new(),
            });
        }
        let last = list.items.last_mut()?;
        last.raw = js_trim_end(&last.raw).to_string();
        last.text = js_trim_end(&last.text).to_string();
        let list_raw = js_trim_end(&list_raw).to_string();
        for i in 0..list.items.len() {
            self.state.top = false;
            let text = list.items[i].text.clone();
            let mut tokens = Vec::new();
            self.block_tokens(&text, &mut tokens, false);
            if !list.loose {
                let mut spacers = tokens
                    .iter()
                    .filter(|t| t.kind == TokenType::Space)
                    .peekable();
                list.loose =
                    spacers.peek().is_some() && spacers.any(|t| RULES.other.any_line.test(&t.raw));
            }
            list.items[i].tokens = tokens;
        }
        if list.loose {
            for item in &mut list.items {
                item.loose = true;
            }
        }
        let mut t = Token::new(TokenType::List, list_raw);
        t.list = Some(Box::new(list));
        Some(t)
    }

    fn html(&self, src: &str) -> Option<Token> {
        let cap = RULES.block.html.exec(src)?;
        let mut t = Token::new(TokenType::Html, cap.whole()).with_text(cap.whole());
        t.block = Some(true);
        t.pre = Some(matches!(cap.get(1), Some("pre" | "script" | "style")));
        Some(t)
    }

    fn def(&self, src: &str) -> Option<(String, String, LinkDef)> {
        let cap = RULES.block.def.exec(src)?;
        let tag = RULES.other.multiple_space_global.replace(
            &cap.get(1).unwrap().to_lowercase(),
            true,
            " ",
        );
        let href = match cap.truthy(2) {
            Some(h) => unescape_punct(&RULES.other.href_brackets.replace(h, false, "$1")),
            None => String::new(),
        };
        let title = cap.truthy(3).map(|t| {
            let inner: String = {
                let mut cs = t.chars();
                cs.next();
                cs.next_back();
                cs.collect()
            };
            unescape_punct(&inner)
        });
        Some((cap.whole().to_string(), tag, LinkDef { href, title }))
    }

    fn table(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.table.exec(src)?;
        let cap2 = cap.get(2).unwrap_or("");
        if !RULES.other.table_delimiter.test(cap2) {
            return None;
        }
        let headers = split_cells(cap.get(1).unwrap(), None);
        let aligns: Vec<String> = RULES
            .other
            .table_align_chars
            .replace(cap2, true, "")
            .split('|')
            .map(str::to_string)
            .collect();
        let rows: Vec<String> = match cap.get(3) {
            Some(r) if !js_trim(r).is_empty() => RULES
                .other
                .table_row_blank_line
                .replace(r, false, "")
                .split('\n')
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        };
        if headers.len() != aligns.len() {
            return None;
        }
        let mut table = TableData::default();
        for align in &aligns {
            table
                .align
                .push(if RULES.other.table_align_right.test(align) {
                    Some(Align::Right)
                } else if RULES.other.table_align_center.test(align) {
                    Some(Align::Center)
                } else if RULES.other.table_align_left.test(align) {
                    Some(Align::Left)
                } else {
                    None
                });
        }
        for (i, h) in headers.iter().enumerate() {
            let slot = self.queue_inline(h);
            table.header.push(TableCell {
                text: h.clone(),
                tokens: Vec::new(),
                header: true,
                align: table.align[i],
                slot: Some(slot),
            });
        }
        for row in &rows {
            let cells = split_cells(row, Some(table.header.len()));
            let mut out = Vec::new();
            for (i, cell) in cells.into_iter().enumerate() {
                let slot = self.queue_inline(&cell);
                out.push(TableCell {
                    text: cell,
                    tokens: Vec::new(),
                    header: false,
                    align: table.align.get(i).copied().flatten(),
                    slot: Some(slot),
                });
            }
            table.rows.push(out);
        }
        let mut t = Token::new(TokenType::Table, cap.whole());
        t.table = Some(Box::new(table));
        Some(t)
    }

    fn lheading(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.lheading.exec(src)?;
        let text = cap.get(1).unwrap();
        let mut t = Token::new(TokenType::Heading, cap.whole()).with_text(text);
        t.depth = Some(if cap.get(2).unwrap().starts_with('=') {
            1
        } else {
            2
        });
        t.slot = Some(self.queue_inline(text));
        Some(t)
    }

    fn paragraph(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.paragraph.exec(src)?;
        let c1 = cap.get(1).unwrap();
        let text = c1.strip_suffix('\n').unwrap_or(c1);
        let mut t = Token::new(TokenType::Paragraph, cap.whole()).with_text(text);
        t.slot = Some(self.queue_inline(text));
        Some(t)
    }

    fn block_text(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.block.text.exec(src)?;
        let mut t = Token::new(TokenType::Text, cap.whole()).with_text(cap.whole());
        t.slot = Some(self.queue_inline(cap.whole()));
        Some(t)
    }

    fn block_tokens(
        &mut self,
        src: &str,
        tokens: &mut Vec<Token>,
        mut last_paragraph_clipped: bool,
    ) {
        let mut src = src;
        while !src.is_empty() {
            if let Some(token) = self.space(src) {
                src = advance(src, token.raw.len());
                match tokens.last_mut() {
                    Some(last) if token.raw.len() == 1 => last.raw.push('\n'),
                    _ => tokens.push(token),
                }
                continue;
            }
            if let Some(token) = self.code(src) {
                src = advance(src, token.raw.len());
                match tokens.last_mut() {
                    Some(last) if matches!(last.kind, TokenType::Paragraph | TokenType::Text) => {
                        last.raw.push('\n');
                        last.raw.push_str(&token.raw);
                        let text = format!("{}\n{}", last.text(), token.text());
                        last.text = Some(text.clone());
                        if let Some(q) = self.inline_queue.last_mut() {
                            q.0 = text;
                        }
                    }
                    _ => tokens.push(token),
                }
                continue;
            }
            if let Some(token) = self.fences(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some(token) = self.heading(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some(token) = self.hr(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some(token) = self.blockquote(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some(token) = self.list(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some(token) = self.html(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some((raw, tag, def)) = self.def(src) {
                src = advance(src, raw.len());
                match tokens.last_mut() {
                    Some(last) if matches!(last.kind, TokenType::Paragraph | TokenType::Text) => {
                        last.raw.push('\n');
                        last.raw.push_str(&raw);
                        let text = format!("{}\n{}", last.text(), raw);
                        last.text = Some(text.clone());
                        if let Some(q) = self.inline_queue.last_mut() {
                            q.0 = text;
                        }
                    }
                    _ => {
                        if !self.has_link(&tag) {
                            self.links.push((tag, def));
                        }
                    }
                }
                continue;
            }
            if let Some(token) = self.table(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if let Some(token) = self.lheading(src) {
                src = advance(src, token.raw.len());
                tokens.push(token);
                continue;
            }
            if self.state.top {
                if let Some(token) = self.paragraph(src) {
                    let raw_len = token.raw.len();
                    match tokens.last_mut() {
                        Some(last)
                            if last_paragraph_clipped && last.kind == TokenType::Paragraph =>
                        {
                            last.raw.push('\n');
                            last.raw.push_str(&token.raw);
                            let text = format!("{}\n{}", last.text(), token.text());
                            last.text = Some(text.clone());
                            self.inline_queue.pop();
                            if let Some(q) = self.inline_queue.last_mut() {
                                q.0 = text;
                            }
                        }
                        _ => tokens.push(token),
                    }
                    last_paragraph_clipped = false;
                    src = advance(src, raw_len);
                    continue;
                }
            }
            if let Some(token) = self.block_text(src) {
                src = advance(src, token.raw.len());
                match tokens.last_mut() {
                    Some(last) if last.kind == TokenType::Text => {
                        last.raw.push('\n');
                        last.raw.push_str(&token.raw);
                        let text = format!("{}\n{}", last.text(), token.text());
                        last.text = Some(text.clone());
                        self.inline_queue.pop();
                        if let Some(q) = self.inline_queue.last_mut() {
                            q.0 = text;
                        }
                    }
                    _ => tokens.push(token),
                }
                continue;
            }
            // `marked` throws "Infinite loop on byte"; the rules above always
            // consume at least one character, so this is unreachable.
            break;
        }
        self.state.top = true;
    }

    // ---- inline tokenizers -----------------------------------------------

    fn output_link(
        &mut self,
        cap0: &str,
        cap1: &str,
        href: String,
        title: Option<String>,
        raw: &str,
    ) -> Token {
        let text = RULES.other.output_link_replace.replace(cap1, true, "$1");
        self.state.in_link = true;
        let kind = if cap0.starts_with('!') {
            TokenType::Image
        } else {
            TokenType::Link
        };
        let mut t = Token::new(kind, raw).with_text(text.clone());
        t.href = Some(href);
        t.title = Some(title.filter(|t| !t.is_empty()));
        t.tokens = Some(self.inline_tokens(&text));
        self.state.in_link = false;
        t
    }

    fn escape(&self, src: &str) -> Option<Token> {
        let cap = RULES.inline.escape.exec(src)?;
        Some(Token::new(TokenType::Escape, cap.whole()).with_text(cap.get(1).unwrap()))
    }

    fn tag(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.inline.tag.exec(src)?;
        let raw = cap.whole();
        if !self.state.in_link && RULES.other.start_a_tag.test(raw) {
            self.state.in_link = true;
        } else if self.state.in_link && RULES.other.end_a_tag.test(raw) {
            self.state.in_link = false;
        }
        if !self.state.in_raw_block && RULES.other.start_pre_script_tag.test(raw) {
            self.state.in_raw_block = true;
        } else if self.state.in_raw_block && RULES.other.end_pre_script_tag.test(raw) {
            self.state.in_raw_block = false;
        }
        let mut t = Token::new(TokenType::Html, raw).with_text(raw);
        t.in_link = Some(self.state.in_link);
        t.in_raw_block = Some(self.state.in_raw_block);
        t.block = Some(false);
        Some(t)
    }

    fn link(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.inline.link.exec(src)?;
        let mut cap0 = cap.whole().to_string();
        let cap1 = cap.get(1).unwrap().to_string();
        let mut cap2 = cap.get(2).unwrap().to_string();
        let mut cap3 = cap.get(3).map(str::to_string);
        let trimmed_url = js_trim(&cap2).to_string();
        if RULES.other.start_angle_bracket.test(&trimmed_url) {
            if !RULES.other.end_angle_bracket.test(&trimmed_url) {
                return None;
            }
            let rtrim_slash = rtrim(&trimmed_url[..trimmed_url.len() - 1], '\\', false);
            if (trimmed_url.len() - rtrim_slash.len()).is_multiple_of(2) {
                return None;
            }
        } else {
            let last_paren = find_closing_paren(&cap2);
            if last_paren == -2 {
                return None;
            }
            if last_paren > -1 {
                let last_paren = last_paren as usize;
                let start = if cap0.starts_with('!') { 5 } else { 4 };
                let link_len = start + cap1.len() + last_paren;
                cap2.truncate(last_paren);
                cap0 = js_trim(cap0.get(..link_len).unwrap_or(&cap0)).to_string();
                cap3 = Some(String::new());
            }
        }
        let mut href = cap2.clone();
        let title = match cap3.as_deref() {
            Some(t) if !t.is_empty() => {
                let mut cs = t.chars();
                cs.next();
                cs.next_back();
                cs.as_str().to_string()
            }
            _ => String::new(),
        };
        href = js_trim(&href).to_string();
        if RULES.other.start_angle_bracket.test(&href) {
            href = if href.len() >= 2 {
                href[1..href.len() - 1].to_string()
            } else {
                String::new()
            };
        }
        let href = if href.is_empty() {
            href
        } else {
            unescape_punct(&href)
        };
        let title = if title.is_empty() {
            title
        } else {
            unescape_punct(&title)
        };
        Some(self.output_link(&cap0.clone(), &cap1, href, Some(title), &cap0))
    }

    fn reflink(&mut self, src: &str) -> Option<Token> {
        let cap = RULES
            .inline
            .reflink
            .exec(src)
            .or_else(|| RULES.inline.nolink.exec(src))?;
        let link_string = cap.truthy(2).or(cap.get(1)).unwrap_or("");
        let link_string = RULES
            .other
            .multiple_space_global
            .replace(link_string, true, " ");
        let key = link_string.to_lowercase();
        let Some(def) = self
            .links
            .iter()
            .find(|(t, _)| *t == key)
            .map(|(_, d)| d.clone())
        else {
            let text: String = cap.whole().chars().take(1).collect();
            return Some(Token::new(TokenType::Text, text.clone()).with_text(text));
        };
        let cap0 = cap.whole().to_string();
        let cap1 = cap.get(1).unwrap_or("").to_string();
        Some(self.output_link(&cap0, &cap1, def.href, def.title, &cap0))
    }

    fn em_strong(&mut self, src: &str, masked_src: &str, prev_char: PrevChar) -> Option<Token> {
        let m = RULES.inline.em_strong_l_delim.exec(src)?;
        if m.get(3).is_some_and(|s| !s.is_empty()) && prev_char.is_alnum() {
            return None;
        }
        let next_char = m.truthy(1).or(m.truthy(2)).unwrap_or("");
        if !(next_char.is_empty() || prev_char.is_empty() || prev_char.is_punctuation()) {
            return None;
        }
        let l_length = m.whole().chars().count() - 1;
        let mut delim_total = l_length as isize;
        let mut mid_delim_total = 0isize;
        let end_reg = if m.whole().starts_with('*') {
            &RULES.inline.em_strong_r_delim_ast
        } else {
            &RULES.inline.em_strong_r_delim_und
        };
        let offset = masked_src.len() - src.len() + l_length;
        let masked = &masked_src[offset..];
        let mut pos = 0;
        while let Some(m) = end_reg.exec_at(masked, pos) {
            pos = if m.end() > m.index() {
                m.end()
            } else {
                m.end() + 1
            };
            let r_delim = (1..=6).find_map(|i| m.truthy(i));
            let Some(r_delim) = r_delim else { continue };
            let mut r_length = r_delim.chars().count() as isize;
            if m.truthy(3).is_some() || m.truthy(4).is_some() {
                delim_total += r_length;
                continue;
            } else if (m.truthy(5).is_some() || m.truthy(6).is_some())
                && l_length % 3 != 0
                && (l_length as isize + r_length) % 3 == 0
            {
                mid_delim_total += r_length;
                continue;
            }
            delim_total -= r_length;
            if delim_total > 0 {
                continue;
            }
            r_length = r_length.min(r_length + delim_total + mid_delim_total);
            let last_char_length = m.whole().chars().next().map_or(0, char::len_utf8);
            let end = l_length + m.index() + last_char_length + r_length as usize;
            let raw = &src[..end.min(src.len())];
            if (l_length as isize).min(r_length) % 2 == 1 {
                let text = &raw[1..raw.len() - 1];
                let tokens = self.inline_tokens(text);
                return Some(
                    Token::new(TokenType::Em, raw)
                        .with_text(text)
                        .with_tokens(tokens),
                );
            }
            let text = &raw[2..raw.len() - 2];
            let tokens = self.inline_tokens(text);
            return Some(
                Token::new(TokenType::Strong, raw)
                    .with_text(text)
                    .with_tokens(tokens),
            );
        }
        None
    }

    fn codespan(&self, src: &str) -> Option<Token> {
        let cap = RULES.inline.code.exec(src)?;
        let mut text = cap.get(2).unwrap().replace('\n', " ");
        let has_non_space = RULES.other.non_space_char.test(&text);
        let both_ends = text.starts_with(' ') && text.ends_with(' ');
        if has_non_space && both_ends {
            text = text[1..text.len() - 1].to_string();
        }
        Some(Token::new(TokenType::Codespan, cap.whole()).with_text(text))
    }

    fn br(&self, src: &str) -> Option<Token> {
        let cap = RULES.inline.br.exec(src)?;
        Some(Token::new(TokenType::Br, cap.whole()))
    }

    fn del(&mut self, src: &str) -> Option<Token> {
        let cap = RULES.inline.del.exec(src)?;
        let text = cap.get(2).unwrap();
        let tokens = self.inline_tokens(text);
        Some(
            Token::new(TokenType::Del, cap.whole())
                .with_text(text)
                .with_tokens(tokens),
        )
    }

    fn autolink(&self, src: &str) -> Option<Token> {
        let cap = RULES.inline.autolink.exec(src)?;
        let text = cap.get(1).unwrap().to_string();
        let href = if cap.get(2) == Some("@") {
            format!("mailto:{text}")
        } else {
            text.clone()
        };
        let mut t = Token::new(TokenType::Link, cap.whole()).with_text(text.clone());
        t.href = Some(href);
        t.tokens = Some(vec![
            Token::new(TokenType::Text, text.clone()).with_text(text)
        ]);
        Some(t)
    }

    fn url(&self, src: &str) -> Option<Token> {
        let cap = RULES.inline.url.exec(src)?;
        let (raw, href) = if cap.get(2) == Some("@") {
            let text = cap.whole().to_string();
            (text.clone(), format!("mailto:{text}"))
        } else {
            let mut cap0 = cap.whole().to_string();
            loop {
                let prev = cap0.clone();
                cap0 = RULES
                    .inline
                    .backpedal
                    .exec(&cap0)
                    .map(|m| m.whole().to_string())
                    .unwrap_or_default();
                if prev == cap0 {
                    break;
                }
            }
            let href = if cap.get(1) == Some("www.") {
                format!("http://{cap0}")
            } else {
                cap0.clone()
            };
            (cap0, href)
        };
        let mut t = Token::new(TokenType::Link, raw.clone()).with_text(raw.clone());
        t.href = Some(href);
        t.tokens = Some(vec![Token::new(TokenType::Text, raw.clone()).with_text(raw)]);
        Some(t)
    }

    fn inline_text(&self, src: &str) -> Option<Token> {
        let cap = RULES.inline.text.exec(src)?;
        let mut t = Token::new(TokenType::Text, cap.whole()).with_text(cap.whole());
        t.escaped = Some(self.state.in_raw_block);
        Some(t)
    }

    fn inline_tokens(&mut self, src: &str) -> Vec<Token> {
        let mut tokens: Vec<Token> = Vec::new();
        let mut masked = src.to_string();
        if !self.links.is_empty() {
            let re = &RULES.inline.reflink_search;
            let mut pos = 0;
            while let Some(m) = re.exec_at(&masked, pos) {
                let (start, end) = (m.index(), m.end());
                let whole = m.whole();
                let label = &whole[whole.rfind('[').unwrap() + 1..whole.len() - 1];
                let hit = self.has_link(label);
                pos = end;
                if hit {
                    mask(&mut masked, start, end, "a");
                }
            }
        }
        let mut pos = 0;
        while let Some(m) = RULES.inline.any_punctuation.exec_at(&masked, pos) {
            let (start, end) = (m.index(), m.end());
            pos = end;
            mask(&mut masked, start, end, "+");
        }
        let mut pos = 0;
        while let Some(m) = RULES.inline.block_skip.exec_at(&masked, pos) {
            let (start, end) = (m.index(), m.end());
            pos = end;
            mask(&mut masked, start, end, "a");
        }

        let mut src = src;
        let mut keep_prev_char = false;
        let mut prev_char = PrevChar::Empty;
        while !src.is_empty() {
            if !keep_prev_char {
                prev_char = PrevChar::Empty;
            }
            keep_prev_char = false;
            if let Some(t) = self.escape(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.tag(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.link(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.reflink(src) {
                src = advance(src, t.raw.len());
                match tokens.last_mut() {
                    Some(last) if t.kind == TokenType::Text && last.kind == TokenType::Text => {
                        last.raw.push_str(&t.raw);
                        let text = format!("{}{}", last.text(), t.text());
                        last.text = Some(text);
                    }
                    _ => tokens.push(t),
                }
                continue;
            }
            if let Some(t) = self.em_strong(src, &masked, prev_char) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.codespan(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.br(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.del(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if let Some(t) = self.autolink(src) {
                src = advance(src, t.raw.len());
                tokens.push(t);
                continue;
            }
            if !self.state.in_link {
                if let Some(t) = self.url(src) {
                    src = advance(src, t.raw.len());
                    tokens.push(t);
                    continue;
                }
            }
            if let Some(t) = self.inline_text(src) {
                src = advance(src, t.raw.len());
                if !t.raw.ends_with('_') {
                    prev_char = PrevChar::from_last(&t.raw);
                }
                keep_prev_char = true;
                match tokens.last_mut() {
                    Some(last) if last.kind == TokenType::Text => {
                        last.raw.push_str(&t.raw);
                        let text = format!("{}{}", last.text(), t.text());
                        last.text = Some(text);
                    }
                    _ => tokens.push(t),
                }
                continue;
            }
            break;
        }
        tokens
    }
}

/// `prevChar` in `inlineTokens`: the last UTF-16 unit of the previous text
/// run. An astral character leaves a lone surrogate, which matches neither
/// the alphanumeric nor the punctuation test.
#[derive(Debug, Clone, Copy)]
enum PrevChar {
    Empty,
    Char(char),
    LoneSurrogate,
}

impl PrevChar {
    fn from_last(s: &str) -> Self {
        match s.chars().next_back() {
            None => Self::Empty,
            Some(c) if c.len_utf16() == 2 => Self::LoneSurrogate,
            Some(c) => Self::Char(c),
        }
    }

    fn is_empty(self) -> bool {
        matches!(self, Self::Empty)
    }

    fn as_str(self) -> Option<String> {
        match self {
            Self::Char(c) => Some(c.to_string()),
            _ => None,
        }
    }

    fn is_alnum(self) -> bool {
        self.as_str()
            .is_some_and(|s| RULES.other.unicode_alpha_numeric.test(&s))
    }

    fn is_punctuation(self) -> bool {
        self.as_str()
            .is_some_and(|s| RULES.inline.punctuation.test(&s))
    }
}

// ---- JSON dump -------------------------------------------------------------
//
// `JSON.stringify` of `marked`'s tokens with keys sorted and `undefined`
// fields dropped — the form the golden corpus stores, so a token stream can
// be compared with the real `marked` byte for byte.

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_obj(fields: Vec<(&str, String)>) -> String {
    let mut fields = fields;
    fields.sort_by(|a, b| a.0.cmp(b.0));
    let body: Vec<String> = fields
        .into_iter()
        .map(|(k, v)| format!("{}:{v}", json_str(k)))
        .collect();
    format!("{{{}}}", body.join(","))
}

fn json_opt_str(s: &Option<String>) -> String {
    s.as_deref().map_or_else(|| "null".to_string(), json_str)
}

fn json_align(a: Option<Align>) -> String {
    match a {
        None => "null".into(),
        Some(Align::Left) => json_str("left"),
        Some(Align::Center) => json_str("center"),
        Some(Align::Right) => json_str("right"),
    }
}

fn json_cell(c: &TableCell) -> String {
    json_obj(vec![
        ("align", json_align(c.align)),
        ("header", c.header.to_string()),
        ("text", json_str(&c.text)),
        ("tokens", tokens_to_json(&c.tokens)),
    ])
}

/// A token as `JSON.stringify` (sorted keys) prints `marked`'s object.
pub fn token_to_json(t: &Token) -> String {
    let mut f: Vec<(&str, String)> = vec![
        ("type", json_str(t.kind.as_str())),
        ("raw", json_str(&t.raw)),
    ];
    if let Some(text) = &t.text {
        f.push(("text", json_str(text)));
    }
    if let Some(tokens) = &t.tokens {
        f.push(("tokens", tokens_to_json(tokens)));
    }
    if let Some(d) = t.depth {
        f.push(("depth", d.to_string()));
    }
    if let Some(lang) = &t.lang {
        f.push(("lang", json_str(lang)));
    }
    if t.indented {
        f.push(("codeBlockStyle", json_str("indented")));
    }
    if let Some(href) = &t.href {
        f.push(("href", json_str(href)));
    }
    if let Some(title) = &t.title {
        f.push(("title", json_opt_str(title)));
    }
    for (k, v) in [
        ("block", t.block),
        ("pre", t.pre),
        ("inLink", t.in_link),
        ("inRawBlock", t.in_raw_block),
        ("escaped", t.escaped),
    ] {
        if let Some(v) = v {
            f.push((k, v.to_string()));
        }
    }
    if let Some(list) = &t.list {
        f.push(("ordered", list.ordered.to_string()));
        f.push((
            "start",
            list.start.map_or_else(|| json_str(""), |n| n.to_string()),
        ));
        f.push(("loose", list.loose.to_string()));
        let items: Vec<String> = list
            .items
            .iter()
            .map(|i| {
                let mut g = vec![
                    ("type", json_str("list_item")),
                    ("raw", json_str(&i.raw)),
                    ("task", i.task.to_string()),
                    ("loose", i.loose.to_string()),
                    ("text", json_str(&i.text)),
                    ("tokens", tokens_to_json(&i.tokens)),
                ];
                if let Some(c) = i.checked {
                    g.push(("checked", c.to_string()));
                }
                json_obj(g)
            })
            .collect();
        f.push(("items", format!("[{}]", items.join(","))));
    }
    if let Some(table) = &t.table {
        let align: Vec<String> = table.align.iter().map(|a| json_align(*a)).collect();
        f.push(("align", format!("[{}]", align.join(","))));
        let header: Vec<String> = table.header.iter().map(json_cell).collect();
        f.push(("header", format!("[{}]", header.join(","))));
        let rows: Vec<String> = table
            .rows
            .iter()
            .map(|r| {
                format!(
                    "[{}]",
                    r.iter().map(json_cell).collect::<Vec<_>>().join(",")
                )
            })
            .collect();
        f.push(("rows", format!("[{}]", rows.join(","))));
    }
    json_obj(f)
}

/// A token list as `JSON.stringify` (sorted keys) prints it.
pub fn tokens_to_json(tokens: &[Token]) -> String {
    format!(
        "[{}]",
        tokens
            .iter()
            .map(token_to_json)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// `Object.entries(tokens.links)` with sorted keys, as JSON.
pub fn links_to_json(links: &[(String, LinkDef)]) -> String {
    let entries: Vec<String> = links
        .iter()
        .map(|(tag, def)| {
            let mut f = vec![("href", json_str(&def.href))];
            if let Some(title) = &def.title {
                f.push(("title", json_str(title)));
            }
            format!("[{},{}]", json_str(tag), json_obj(f))
        })
        .collect();
    format!("[{}]", entries.join(","))
}
