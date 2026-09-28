//! The compact startup banner (`core/wordmark.ts`).

/// The three-line owl glyph beside the brand text.
const WORDMARK_GLYPH: [&str; 3] = ["▟▀▀▀▀▀▙", "▌▟▙ ▟▙▐", "▜▄▄▄▄▄▛"];

const GLYPH_GAP: &str = "  ";

type Style<'a> = &'a dyn Fn(&str) -> String;

/// `CompactWordmarkOptions`.
pub struct CompactWordmarkOptions<'a> {
    pub app_name: &'a str,
    pub version: &'a str,
    pub cwd: &'a str,
    /// Tagline next to the version (default "coding agent").
    pub tagline: Option<&'a str>,
    /// The brand name.
    pub accent: Style<'a>,
    /// The owl glyph; defaults to `accent`.
    pub glyph: Option<Style<'a>>,
    /// Secondary text (tagline, version, cwd).
    pub dim: Style<'a>,
    /// Separators.
    pub muted: Style<'a>,
    /// The trailing blinking cursor, when set.
    pub cursor: Option<Style<'a>>,
    /// A note appended to the cwd line (e.g. a keybinding hint).
    pub note: Option<&'a dyn Fn() -> String>,
}

/// `buildCompactWordmark`: glyph beside brand, tagline + version, and cwd,
/// flush against the left edge.
pub fn build_compact_wordmark(options: &CompactWordmarkOptions<'_>) -> String {
    let accent = options.accent;
    let glyph_style = options.glyph.unwrap_or(accent);
    let tagline = options.tagline.unwrap_or("coding agent");

    // Highlight the "hoo" prefix when present, otherwise accent the whole name.
    let name = match options.app_name.strip_prefix("hoo") {
        Some(rest) => format!("{}{}{rest}", accent("hoo"), (options.muted)("│")),
        None => accent(options.app_name),
    };
    let brand = match options.cursor {
        Some(cursor) => format!("{name}{}", cursor("_")),
        None => name,
    };
    let dim = options.dim;
    let right = [
        brand,
        format!(
            "{} {} {}",
            dim(tagline),
            (options.muted)("·"),
            dim(&format!("v{}", options.version))
        ),
        match options.note {
            Some(note) => format!("{}{}", dim(options.cwd), note()),
            None => dim(options.cwd),
        },
    ];
    WORDMARK_GLYPH
        .iter()
        .zip(right.iter())
        .map(|(glyph, text)| format!("{}{GLYPH_GAP}{text}", glyph_style(glyph)))
        .collect::<Vec<_>>()
        .join("\n")
}
