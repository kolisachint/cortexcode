//! Compiled `marked` rules (GFM, no `breaks`, not pedantic) plus the
//! tokenizer's helper regexes (`other` in `marked`'s `rules.ts`).

use once_cell::sync::Lazy;

use super::js_regex::JsRegex;
use super::rules_gen as g;

fn re((source, flags): (&str, &str)) -> JsRegex {
    JsRegex::new(source, flags)
}

pub struct Block {
    pub blockquote: JsRegex,
    pub code: JsRegex,
    pub def: JsRegex,
    pub fences: JsRegex,
    pub heading: JsRegex,
    pub hr: JsRegex,
    pub html: JsRegex,
    pub lheading: JsRegex,
    pub list: JsRegex,
    pub newline: JsRegex,
    pub paragraph: JsRegex,
    pub table: JsRegex,
    pub text: JsRegex,
}

pub struct Inline {
    pub backpedal: JsRegex,
    pub any_punctuation: JsRegex,
    pub autolink: JsRegex,
    pub block_skip: JsRegex,
    pub br: JsRegex,
    pub code: JsRegex,
    /// markdown.ts's `STRICT_STRIKETHROUGH_REGEX` (replaces GFM `del`).
    pub del: JsRegex,
    pub em_strong_l_delim: JsRegex,
    pub em_strong_r_delim_ast: JsRegex,
    pub em_strong_r_delim_und: JsRegex,
    pub escape: JsRegex,
    pub link: JsRegex,
    pub nolink: JsRegex,
    pub punctuation: JsRegex,
    pub reflink: JsRegex,
    pub reflink_search: JsRegex,
    pub tag: JsRegex,
    pub text: JsRegex,
    pub url: JsRegex,
}

pub struct Other {
    pub code_remove_indent: JsRegex,
    pub output_link_replace: JsRegex,
    pub indent_code_compensation: JsRegex,
    pub beginning_space: JsRegex,
    pub ending_hash: JsRegex,
    pub ending_space_char: JsRegex,
    pub non_space_char: JsRegex,
    pub blank_line: JsRegex,
    pub double_blank_line: JsRegex,
    pub blockquote_start: JsRegex,
    pub blockquote_setext_replace: JsRegex,
    pub blockquote_setext_replace2: JsRegex,
    pub list_replace_tabs: JsRegex,
    pub list_is_task: JsRegex,
    pub list_replace_task: JsRegex,
    pub any_line: JsRegex,
    pub href_brackets: JsRegex,
    pub table_delimiter: JsRegex,
    pub table_align_chars: JsRegex,
    pub table_row_blank_line: JsRegex,
    pub table_align_right: JsRegex,
    pub table_align_center: JsRegex,
    pub table_align_left: JsRegex,
    pub start_a_tag: JsRegex,
    pub end_a_tag: JsRegex,
    pub start_pre_script_tag: JsRegex,
    pub end_pre_script_tag: JsRegex,
    pub start_angle_bracket: JsRegex,
    pub end_angle_bracket: JsRegex,
    pub unicode_alpha_numeric: JsRegex,
    pub multiple_space_global: JsRegex,
    pub carriage_return: JsRegex,
}

pub struct Rules {
    pub block: Block,
    pub inline: Inline,
    pub other: Other,
}

pub static RULES: Lazy<Rules> = Lazy::new(|| Rules {
    block: Block {
        blockquote: re(g::BLOCK_BLOCKQUOTE),
        code: re(g::BLOCK_CODE),
        def: re(g::BLOCK_DEF),
        fences: re(g::BLOCK_FENCES),
        heading: re(g::BLOCK_HEADING),
        hr: re(g::BLOCK_HR),
        html: re(g::BLOCK_HTML),
        lheading: re(g::BLOCK_LHEADING),
        list: re(g::BLOCK_LIST),
        newline: re(g::BLOCK_NEWLINE),
        paragraph: re(g::BLOCK_PARAGRAPH),
        table: re(g::BLOCK_TABLE),
        text: re(g::BLOCK_TEXT),
    },
    inline: Inline {
        backpedal: re(g::INLINE_BACKPEDAL),
        any_punctuation: re(g::INLINE_ANY_PUNCTUATION),
        autolink: re(g::INLINE_AUTOLINK),
        block_skip: re(g::INLINE_BLOCK_SKIP),
        br: re(g::INLINE_BR),
        code: re(g::INLINE_CODE),
        del: JsRegex::new(
            r"^(~~)(?=[^\s~])((?:\\.|[^\\])*?(?:\\.|[^\s~\\]))\1(?=[^~]|$)",
            "",
        ),
        em_strong_l_delim: re(g::INLINE_EM_STRONG_LDELIM),
        em_strong_r_delim_ast: re(g::INLINE_EM_STRONG_RDELIM_AST),
        em_strong_r_delim_und: re(g::INLINE_EM_STRONG_RDELIM_UND),
        escape: re(g::INLINE_ESCAPE),
        link: re(g::INLINE_LINK),
        nolink: re(g::INLINE_NOLINK),
        punctuation: re(g::INLINE_PUNCTUATION),
        reflink: re(g::INLINE_REFLINK),
        reflink_search: re(g::INLINE_REFLINK_SEARCH),
        tag: re(g::INLINE_TAG),
        text: re(g::INLINE_TEXT),
        url: re(g::INLINE_URL),
    },
    other: Other {
        code_remove_indent: JsRegex::new(r"^(?: {1,4}| {0,3}\t)", "gm"),
        output_link_replace: JsRegex::new(r"\\([\[\]])", "g"),
        indent_code_compensation: JsRegex::new(r"^(\s+)(?:```)", ""),
        beginning_space: JsRegex::new(r"^\s+", ""),
        ending_hash: JsRegex::new(r"#$", ""),
        ending_space_char: JsRegex::new(r" $", ""),
        non_space_char: JsRegex::new(r"[^ ]", ""),
        blank_line: JsRegex::new(r"^[ \t]*$", ""),
        double_blank_line: JsRegex::new(r"\n[ \t]*\n[ \t]*$", ""),
        blockquote_start: JsRegex::new(r"^ {0,3}>", ""),
        blockquote_setext_replace: JsRegex::new(r"\n {0,3}((?:=+|-+) *)(?=\n|$)", "g"),
        blockquote_setext_replace2: JsRegex::new(r"^ {0,3}>[ \t]?", "gm"),
        list_replace_tabs: JsRegex::new(r"^\t+", ""),
        list_is_task: JsRegex::new(r"^\[[ xX]\] ", ""),
        list_replace_task: JsRegex::new(r"^\[[ xX]\] +", ""),
        any_line: JsRegex::new(r"\n.*\n", ""),
        href_brackets: JsRegex::new(r"^<(.*)>$", ""),
        table_delimiter: JsRegex::new(r"[:|]", ""),
        table_align_chars: JsRegex::new(r"^\||\| *$", "g"),
        table_row_blank_line: JsRegex::new(r"\n[ \t]*$", ""),
        table_align_right: JsRegex::new(r"^ *-+: *$", ""),
        table_align_center: JsRegex::new(r"^ *:-+: *$", ""),
        table_align_left: JsRegex::new(r"^ *:-+ *$", ""),
        start_a_tag: JsRegex::new(r"^<a ", "i"),
        end_a_tag: JsRegex::new(r"^<\/a>", "i"),
        start_pre_script_tag: JsRegex::new(r"^<(pre|code|kbd|script)(\s|>)", "i"),
        end_pre_script_tag: JsRegex::new(r"^<\/(pre|code|kbd|script)(\s|>)", "i"),
        start_angle_bracket: JsRegex::new(r"^<", ""),
        end_angle_bracket: JsRegex::new(r">$", ""),
        unicode_alpha_numeric: JsRegex::new(r"[\p{L}\p{N}]", "u"),
        multiple_space_global: JsRegex::new(r"\s+", "g"),
        carriage_return: JsRegex::new(r"\r\n|\r", "g"),
    },
});

/// The per-list-item regexes `marked` builds from the item's indent.
pub struct IndentRules {
    pub next_bullet: JsRegex,
    pub hr: JsRegex,
    pub fences_begin: JsRegex,
    pub heading_begin: JsRegex,
    pub html_begin: JsRegex,
}

impl IndentRules {
    pub fn new(indent: usize) -> Self {
        let n = 3.min(indent as isize - 1);
        // `{0,-1}` is a literal in JavaScript (not a quantifier): an item at
        // indent 0 can only happen for the pedantic flavour, never here.
        let q = format!("{{0,{n}}}");
        Self {
            next_bullet: JsRegex::new(
                &format!(r"^ {q}(?:[*+-]|\d{{1,9}}[.)])((?:[ 	][^\n]*)?(?:\n|$))"),
                "",
            ),
            hr: JsRegex::new(
                &format!(r"^ {q}((?:- *){{3,}}|(?:_ *){{3,}}|(?:\* *){{3,}})(?:\n+|$)"),
                "",
            ),
            fences_begin: JsRegex::new(&format!(r"^ {q}(?:```|~~~)"), ""),
            heading_begin: JsRegex::new(&format!(r"^ {q}#"), ""),
            html_begin: JsRegex::new(&format!(r"^ {q}<(?:[a-z].*>|!--)"), "i"),
        }
    }
}

/// `other.listItemRegex(bull)`.
pub fn list_item_regex(bull: &str) -> JsRegex {
    JsRegex::new(&format!(r"^( {{0,3}}{bull})((?:[	 ][^\n]*)?(?:\n|$))"), "")
}
