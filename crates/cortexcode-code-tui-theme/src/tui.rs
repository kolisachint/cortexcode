//! TUI helpers from `theme.ts`: the theme hooks handed to library
//! components, code highlighting, paper sheets and message labels.
//!
//! Every returned closure resolves the *current* theme when it runs (as
//! hoocode's `theme` proxy does), so a theme switch reaches components built
//! before it.

use std::sync::{Arc, RwLock};

use cortexcode_tui_components::{
    BoxComponent, ColorFn, EditorTheme, Input, MarkdownTheme, PaperSheet, SelectListTheme,
    SettingsListTheme,
};
use cortexcode_tui_util::{apply_background_to_line, visible_width};

use crate::chalk;
use crate::registry::theme;
use crate::theme::Theme;

/// The `cli-highlight` class → theme token mapping (`buildCliHighlightTheme`).
pub const CLI_HIGHLIGHT_CLASSES: [(&str, &str); 15] = [
    ("keyword", "syntaxKeyword"),
    ("built_in", "syntaxType"),
    ("literal", "syntaxNumber"),
    ("number", "syntaxNumber"),
    ("string", "syntaxString"),
    ("comment", "syntaxComment"),
    ("function", "syntaxFunction"),
    ("title", "syntaxFunction"),
    ("class", "syntaxType"),
    ("type", "syntaxType"),
    ("attr", "syntaxVariable"),
    ("variable", "syntaxVariable"),
    ("params", "syntaxVariable"),
    ("operator", "syntaxOperator"),
    ("punctuation", "syntaxPunctuation"),
];

/// Styles highlight classes with a theme's syntax tokens.
pub struct CliHighlightTheme<'a> {
    theme: &'a Theme,
}

impl<'a> CliHighlightTheme<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }

    /// `text` styled for a highlight class; unchanged for a class the theme
    /// does not map.
    pub fn style(&self, class: &str, text: &str) -> String {
        match CLI_HIGHLIGHT_CLASSES.iter().find(|(c, _)| *c == class) {
            Some((_, token)) => self.theme.fg(token, text),
            None => text.to_string(),
        }
    }
}

/// A syntax highlighter (`cli-highlight` in hoocode; provided by the app).
pub trait CodeHighlighter: Send + Sync {
    /// `supportsLanguage`.
    fn supports_language(&self, lang: &str) -> bool;
    /// Highlight `code` as `lang`, styling classes through `theme`.
    fn highlight(
        &self,
        code: &str,
        lang: &str,
        theme: &CliHighlightTheme<'_>,
    ) -> Result<String, String>;
}

fn highlighter_slot() -> &'static RwLock<Option<Arc<dyn CodeHighlighter>>> {
    static SLOT: RwLock<Option<Arc<dyn CodeHighlighter>>> = RwLock::new(None);
    &SLOT
}

/// Install the highlighter [`highlight_code`] and markdown code blocks use.
/// Without one, every language counts as unsupported.
pub fn set_code_highlighter(highlighter: Option<Arc<dyn CodeHighlighter>>) {
    *highlighter_slot()
        .write()
        .unwrap_or_else(|e| e.into_inner()) = highlighter;
}

fn highlighter_for(lang: Option<&str>) -> Option<(Arc<dyn CodeHighlighter>, String)> {
    let lang = lang.filter(|l| !l.is_empty())?;
    let highlighter = highlighter_slot()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()?;
    highlighter
        .supports_language(lang)
        .then(|| (highlighter, lang.to_string()))
}

fn plain_code_lines(t: &Theme, code: &str) -> Vec<String> {
    code.split('\n')
        .map(|line| t.fg("mdCodeBlock", line))
        .collect()
}

/// `highlightCode`: highlighted lines. Without a supported language, lines
/// are colored `mdCodeBlock` (auto-detection misreads prose); a highlighter
/// failure returns the lines unstyled.
pub fn highlight_code(code: &str, lang: Option<&str>) -> Vec<String> {
    let t = theme();
    let Some((highlighter, lang)) = highlighter_for(lang) else {
        return plain_code_lines(&t, code);
    };
    match highlighter.highlight(code, &lang, &CliHighlightTheme::new(&t)) {
        Ok(out) => out.split('\n').map(str::to_string).collect(),
        Err(_) => code.split('\n').map(str::to_string).collect(),
    }
}

/// `getLanguageFromPath`: a highlight language from a file extension.
pub fn get_language_from_path(file_path: &str) -> Option<&'static str> {
    let ext = file_path.rsplit('.').next()?.to_lowercase();
    if ext.is_empty() {
        return None;
    }
    Some(match ext.as_str() {
        "ts" | "tsx" => "typescript",
        "js" | "jsx" | "mjs" | "cjs" => "javascript",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "swift" => "swift",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "php" => "php",
        "sh" | "bash" | "zsh" => "bash",
        "fish" => "fish",
        "ps1" => "powershell",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "sass" => "sass",
        "less" => "less",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "xml" => "xml",
        "md" | "markdown" => "markdown",
        "dockerfile" => "dockerfile",
        "makefile" => "makefile",
        "cmake" => "cmake",
        "lua" => "lua",
        "perl" => "perl",
        "r" => "r",
        "scala" => "scala",
        "clj" => "clojure",
        "ex" | "exs" => "elixir",
        "erl" => "erlang",
        "hs" => "haskell",
        "ml" => "ocaml",
        "vim" => "vim",
        "graphql" => "graphql",
        "proto" => "protobuf",
        "tf" | "hcl" => "hcl",
        _ => return None,
    })
}

/// `messageLabel`: the `[name]` tag heading a message block, or a tape strip
/// when the theme sets the tape pair.
pub fn message_label(name: &str) -> String {
    let t = theme();
    if t.has_bg("tapeBg") && t.has("tapeText") {
        return t.bg(
            "tapeBg",
            &t.fg("tapeText", &format!("\x1b[1m {name} \x1b[22m")),
        );
    }
    t.fg("customMessageLabel", &format!("\x1b[1m[{name}]\x1b[22m"))
}

/// `getPaperShadowFn`: the shadow pass for a filled block, when the theme has
/// a `paperShadow`.
pub fn get_paper_shadow_fn() -> Option<ColorFn> {
    theme()
        .has("paperShadow")
        .then(|| Box::new(|text: &str| theme().fg("paperShadow", text)) as ColorFn)
}

/// Columns a sheet holds back from the right margin.
pub const PAPER_INSET: usize = 1;

/// `applyPaperSheet`: gutter and shadow for a filled block, decided per
/// frame so a theme switch under blocks already on screen is safe.
pub fn apply_paper_sheet(box_component: &mut BoxComponent) {
    box_component.set_paper(Some(Box::new(|| {
        get_paper_shadow_fn().map(|shadow| PaperSheet {
            shadow: Some(shadow),
            inset: Some(PAPER_INSET),
        })
    })));
}

/// The fills that make a message block (`BlockFill`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFill {
    UserMessageBg,
    CustomMessageBg,
    WarningBg,
    ToolErrorBg,
}

impl BlockFill {
    pub fn token(self) -> &'static str {
        match self {
            BlockFill::UserMessageBg => "userMessageBg",
            BlockFill::CustomMessageBg => "customMessageBg",
            BlockFill::WarningBg => "warningBg",
            BlockFill::ToolErrorBg => "toolErrorBg",
        }
    }
}

/// `applyBlockFill`: fill a message block and give it the paper edge.
pub fn apply_block_fill(box_component: &mut BoxComponent, fill: BlockFill) {
    let token = fill.token();
    box_component.set_bg_fn(Some(Box::new(move |text: &str| theme().bg(token, text))));
    apply_paper_sheet(box_component);
}

/// Whether headings at `level` render as a headline chip (top two levels,
/// when the theme sets the pair). Looked up per call.
fn chipped(level: u8) -> bool {
    let t = theme();
    level <= 2 && t.has_bg("headlineBg") && t.has("headlineText")
}

/// `getMarkdownTheme`.
pub fn get_markdown_theme() -> MarkdownTheme {
    let fg = |token: &'static str| Box::new(move |text: &str| theme().fg(token, text)) as ColorFn;
    MarkdownTheme {
        heading: Box::new(|text: &str, level: u8| {
            theme().fg(
                if chipped(level) {
                    "headlineText"
                } else {
                    "mdHeading"
                },
                text,
            )
        }),
        heading_block: Some(Box::new(|line: &str, level: u8| {
            if chipped(level) {
                theme().bg("headlineBg", &format!(" {line} "))
            } else {
                line.to_string()
            }
        })),
        link: fg("mdLink"),
        link_url: fg("mdLinkUrl"),
        code: fg("mdCode"),
        code_block: fg("mdCodeBlock"),
        code_block_border: fg("mdCodeBlockBorder"),
        quote: fg("mdQuote"),
        quote_border: fg("mdQuoteBorder"),
        hr: fg("mdHr"),
        list_bullet: fg("mdListBullet"),
        bold: Box::new(|text: &str| theme().bold(text)),
        italic: Box::new(|text: &str| theme().italic(text)),
        underline: Box::new(|text: &str| theme().underline(text)),
        strikethrough: Box::new(chalk::strikethrough),
        highlight_code: Some(Box::new(|code: &str, lang: Option<&str>| {
            let t = theme();
            let Some((highlighter, lang)) = highlighter_for(lang) else {
                return plain_code_lines(&t, code);
            };
            match highlighter.highlight(code, &lang, &CliHighlightTheme::new(&t)) {
                Ok(out) => out.split('\n').map(str::to_string).collect(),
                Err(_) => plain_code_lines(&t, code),
            }
        })),
        code_block_indent: None,
    }
}

/// The cursor every picker marks its selected row with.
pub const SELECT_CURSOR: &str = "› ";

/// `styleInput`: an input line's caret in the muted color.
pub fn style_input(input: &mut Input) -> &mut Input {
    input.prompt_color = Box::new(|text: &str| theme().fg("muted", text));
    input
}

/// `SELECT_GUTTER`: the blank indent of an unselected row.
pub fn select_gutter() -> String {
    " ".repeat(visible_width(SELECT_CURSOR))
}

/// `paintSelectedRow`: a selected row as a full-width `selectedBg` band.
pub fn paint_selected_row(line: &str, width: usize) -> String {
    apply_background_to_line(line, width, |text: &str| theme().bg("selectedBg", text))
}

/// `getSelectListTheme`.
pub fn get_select_list_theme() -> SelectListTheme {
    let fg = |token: &'static str| Box::new(move |text: &str| theme().fg(token, text)) as ColorFn;
    SelectListTheme {
        selected_prefix: fg("accent"),
        selected_text: fg("accent"),
        description: fg("muted"),
        scroll_info: fg("muted"),
        no_match: fg("muted"),
        // Unstyled: SelectList passes the cursor through `selected_text`.
        cursor: Some(SELECT_CURSOR.to_string()),
        selected_row: Some(Box::new(|text: &str| theme().bg("selectedBg", text))),
    }
}

/// `getEditorTheme`.
pub fn get_editor_theme() -> EditorTheme {
    EditorTheme {
        border_color: Box::new(|text: &str| theme().fg("borderMuted", text)),
        border_chars: None,
        select_list: Box::new(get_select_list_theme),
    }
}

/// `getSettingsListTheme`.
pub fn get_settings_list_theme() -> SettingsListTheme {
    SettingsListTheme {
        label: Box::new(|text: &str, selected: bool| {
            if selected {
                theme().fg("accent", text)
            } else {
                text.to_string()
            }
        }),
        value: Box::new(|text: &str, selected: bool| {
            theme().fg(if selected { "accent" } else { "muted" }, text)
        }),
        description: Box::new(|text: &str| theme().fg("dim", text)),
        cursor: theme().fg("accent", SELECT_CURSOR),
        hint: Box::new(|text: &str| theme().fg("dim", text)),
        selected_row: Some(Box::new(|text: &str| theme().bg("selectedBg", text))),
    }
}
