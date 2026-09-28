//! Syntax highlighting as the pinned hoocode does it: `cli-highlight` over
//! highlight.js 10.7.3.
//!
//! - [`engine`]: the highlight.js engine (mode compiler, scanner, keywords,
//!   sub-languages, auto-detection), run against highlight.js's own grammars
//!   (`data/hljs-grammars.json`, dumped from the pin).
//! - [`highlight`]: `cli-highlight`'s `highlight`, which colors the token tree
//!   with a caller theme and falls back to its `DEFAULT_THEME`.
//!
//! Grammars are compiled by mutating them in place (as highlight.js does), so
//! each thread keeps its own copy.

pub mod engine;
pub mod value;

use std::collections::HashSet;

use engine::{Node, Registry};
use value::{get, Callback, Grammars, Obj, V};

const GRAMMARS: &str = include_str!("../data/hljs-grammars.json");

struct Embedded {
    grammars: Grammars,
}

impl Registry for Embedded {
    fn language(&self, name: &str) -> Option<(Obj, Vec<Callback>)> {
        let name = name.to_lowercase();
        let find = |n: &str| {
            self.grammars
                .languages
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, o)| o.clone())
        };
        let lang = find(&name).or_else(|| {
            self.grammars
                .aliases
                .iter()
                .find(|(a, _)| *a == name)
                .and_then(|(_, n)| find(n))
        })?;
        let extensions = match get(&lang, "compilerExtensions") {
            V::Arr(items) => items
                .borrow()
                .iter()
                .filter_map(|v| match v {
                    V::Fn(cb) => Some(*cb),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Some((lang, extensions))
    }

    fn language_names(&self) -> Vec<String> {
        self.grammars
            .languages
            .iter()
            .map(|(k, _)| k.clone())
            .collect()
    }

    fn system_symbols(&self) -> &HashSet<String> {
        &self.grammars.system_symbols
    }
}

thread_local! {
    static REGISTRY: Embedded = Embedded {
        grammars: value::load(GRAMMARS),
    };
}

/// `supportsLanguage`: a language name or alias highlight.js knows.
pub fn supports_language(name: &str) -> bool {
    REGISTRY.with(|r| r.language(name).is_some())
}

/// Every language name (`listLanguages`).
pub fn list_languages() -> Vec<String> {
    REGISTRY.with(|r| r.language_names())
}

/// The outcome of highlighting: the token tree, or the code as plain text
/// (highlight.js gave up on it).
#[derive(Debug, Clone, PartialEq)]
pub enum Highlighted {
    Tree(Vec<Node>),
    Plain(String),
}

/// `hljs.highlight(code, { language, ignoreIllegals })` as a token tree;
/// `None` for an unknown language (highlight.js throws).
pub fn highlight_tree(code: &str, language: &str, ignore_illegals: bool) -> Option<Highlighted> {
    REGISTRY.with(|r| {
        let result = engine::highlight_with(r, language, code, ignore_illegals)?;
        Some(if result.plain {
            Highlighted::Plain(code.to_string())
        } else {
            Highlighted::Tree(result.emitter.into_nodes())
        })
    })
}

/// `hljs.highlightAuto(code)` as a token tree, with the detected language.
pub fn highlight_auto_tree(code: &str) -> (Option<String>, Highlighted) {
    REGISTRY.with(|r| {
        let result = engine::highlight_auto(r, code, None);
        let tree = if result.plain {
            Highlighted::Plain(code.to_string())
        } else {
            Highlighted::Tree(result.emitter.into_nodes())
        };
        (result.language, tree)
    })
}

// ---- cli-highlight ---------------------------------------------------------

/// A chalk 4 style chain (cli-highlight's own chalk): `open`s outermost first.
struct Chalk(&'static [(&'static str, &'static str)]);

impl Chalk {
    fn apply(&self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let open_all: String = self.0.iter().map(|(o, _)| *o).collect();
        let close_all: String = self.0.iter().rev().map(|(_, c)| *c).collect();
        let mut s = text.to_string();
        if s.contains('\x1b') {
            // Innermost styler first, then its parents (chalk's
            // `stringReplaceAll` keeps the close and appends the open).
            for (open, close) in self.0.iter().rev() {
                s = s.replace(close, &format!("{close}{open}"));
            }
        }
        if s.contains('\n') {
            let mut out = String::with_capacity(s.len());
            let mut rest = s.as_str();
            while let Some(i) = rest.find('\n') {
                let (line, nl) = match rest[..i].strip_suffix('\r') {
                    Some(line) => (line, "\r\n"),
                    None => (&rest[..i], "\n"),
                };
                out.push_str(line);
                out.push_str(&close_all);
                out.push_str(nl);
                out.push_str(&open_all);
                rest = &rest[i + 1..];
            }
            out.push_str(rest);
            s = out;
        }
        format!("{open_all}{s}{close_all}")
    }
}

const BLUE: (&str, &str) = ("\x1b[34m", "\x1b[39m");
const CYAN: (&str, &str) = ("\x1b[36m", "\x1b[39m");
const GREEN: (&str, &str) = ("\x1b[32m", "\x1b[39m");
const RED: (&str, &str) = ("\x1b[31m", "\x1b[39m");
const YELLOW: (&str, &str) = ("\x1b[33m", "\x1b[39m");
const GREY: (&str, &str) = ("\x1b[90m", "\x1b[39m");
const DIM: (&str, &str) = ("\x1b[2m", "\x1b[22m");
const ITALIC: (&str, &str) = ("\x1b[3m", "\x1b[23m");
const BOLD: (&str, &str) = ("\x1b[1m", "\x1b[22m");
const UNDERLINE: (&str, &str) = ("\x1b[4m", "\x1b[24m");

/// cli-highlight's `DEFAULT_THEME` (entries that are `plain` are left out).
fn default_style(token: &str) -> Option<Chalk> {
    Some(Chalk(match token {
        "keyword" | "literal" | "class" | "name" => &[BLUE],
        "built_in" | "attr" => &[CYAN],
        "type" => &[CYAN, DIM],
        "number" | "comment" | "doctag" | "addition" => &[GREEN],
        "regexp" | "string" | "deletion" => &[RED],
        "function" => &[YELLOW],
        "meta" | "tag" => &[GREY],
        "emphasis" => &[ITALIC],
        "strong" => &[BOLD],
        "link" => &[UNDERLINE],
        _ => return None,
    }))
}

/// A caller theme: the styled text for a token, or `None` when the theme has
/// no entry for it (`DEFAULT_THEME` then applies).
pub type Theme<'a> = &'a dyn Fn(&str, &str) -> Option<String>;

/// The token cli-highlight reads from a class: `/hljs-(\w+)/`.
fn token_of(kind: &str) -> String {
    kind.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

fn style(theme: Theme<'_>, token: &str, text: &str) -> String {
    if let Some(s) = theme(token, text) {
        return s;
    }
    match default_style(token) {
        Some(chalk) => chalk.apply(text),
        None => text.to_string(),
    }
}

/// The HTML round trip (escape, then parse5's input preprocessing) leaves
/// text as is except for line endings and NULs.
fn html_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\0', "")
}

fn colorize_node(node: &Node, theme: Theme<'_>, context: Option<&str>, out: &mut String) {
    match node {
        Node::Text(text) => {
            let text = html_text(text);
            match context {
                None => out.push_str(&style(theme, "default", &text)),
                Some(_) => out.push_str(&text),
            }
        }
        Node::Scope {
            kind,
            sublanguage,
            children,
        } => match kind.as_deref() {
            // A class-less node emits no tag: its children sit in the parent.
            None | Some("") => {
                for child in children {
                    colorize_node(child, theme, context, out);
                }
            }
            // A sub-language tag's class has no `hljs-` prefix.
            Some(_) if *sublanguage => {
                for child in children {
                    colorize_node(child, theme, None, out);
                }
            }
            Some(kind) => {
                let token = token_of(kind);
                if token.is_empty() {
                    for child in children {
                        colorize_node(child, theme, None, out);
                    }
                    return;
                }
                let mut inner = String::new();
                for child in children {
                    colorize_node(child, theme, Some(&token), &mut inner);
                }
                out.push_str(&style(theme, &token, &inner));
            }
        },
    }
}

/// Color a token tree as cli-highlight's `colorize` does.
pub fn colorize(nodes: &[Node], theme: Theme<'_>) -> String {
    let mut out = String::new();
    for node in nodes {
        colorize_node(node, theme, None, &mut out);
    }
    out
}

/// cli-highlight's `highlight(code, { language, ignoreIllegals, theme })`;
/// `None` for an unknown language.
pub fn highlight(
    code: &str,
    language: &str,
    ignore_illegals: bool,
    theme: Theme<'_>,
) -> Option<String> {
    Some(match highlight_tree(code, language, ignore_illegals)? {
        Highlighted::Tree(nodes) => colorize(&nodes, theme),
        Highlighted::Plain(text) => style(theme, "default", &html_text(&text)),
    })
}
