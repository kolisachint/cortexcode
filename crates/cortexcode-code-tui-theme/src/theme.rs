//! The `Theme` class: tokens resolved to ANSI for one color mode.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::chalk;
use crate::color::{
    bg_ansi, chip_fill, fg_ansi, fill_ink, relative_luminance, rendered_color, ColorMode, RawColor,
};

/// Foreground tokens (`ThemeColor`). A theme may also carry tokens beyond
/// these (theme JSON is open), so the API takes token names as strings.
pub const THEME_COLORS: [&str; 57] = [
    "accent",
    "border",
    "borderAccent",
    "borderMuted",
    "success",
    "error",
    "warning",
    "muted",
    "dim",
    "text",
    "thinkingText",
    "userMessageText",
    "customMessageText",
    "customMessageLabel",
    "toolTitle",
    "toolOutput",
    "mdHeading",
    "mdLink",
    "mdLinkUrl",
    "mdCode",
    "mdCodeBlock",
    "mdCodeBlockBorder",
    "mdQuote",
    "mdQuoteBorder",
    "mdHr",
    "mdListBullet",
    "toolDiffAdded",
    "toolDiffRemoved",
    "toolDiffContext",
    "syntaxComment",
    "syntaxKeyword",
    "syntaxFunction",
    "syntaxVariable",
    "syntaxString",
    "syntaxNumber",
    "syntaxType",
    "syntaxOperator",
    "syntaxPunctuation",
    "thinkingOff",
    "thinkingMinimal",
    "thinkingLow",
    "thinkingMedium",
    "thinkingHigh",
    "thinkingXhigh",
    "bashMode",
    "agent1",
    "agent2",
    "agent3",
    "agent4",
    "agent5",
    "agent6",
    "mcp",
    "brandText",
    "paperShadow",
    "halftone",
    "headlineText",
    "tapeText",
];

/// Background tokens (`ThemeBg`).
pub const THEME_BGS: [&str; 11] = [
    "selectedBg",
    "userMessageBg",
    "customMessageBg",
    "toolPendingBg",
    "toolSuccessBg",
    "toolErrorBg",
    "warningBg",
    "brandBg",
    "headlineBg",
    "tapeBg",
    "activeToolBg",
];

/// The agent identity palette, in hash order.
pub const AGENT_COLOR_TOKENS: [&str; 6] =
    ["agent1", "agent2", "agent3", "agent4", "agent5", "agent6"];

/// `agentColorFor`: the stable palette token for an agent type (djb2 over
/// UTF-16 code units).
pub fn agent_color_for(agent_type: &str) -> &'static str {
    let mut hash: u32 = 5381;
    for unit in agent_type.encode_utf16() {
        hash = (hash << 5).wrapping_add(hash).wrapping_add(unit as u32);
    }
    AGENT_COLOR_TOKENS[hash as usize % AGENT_COLOR_TOKENS.len()]
}

/// `sessionColorToken`: the palette token for a session slot (1-6, wrapping).
pub fn session_color_token(slot: i64) -> &'static str {
    let len = AGENT_COLOR_TOKENS.len() as i64;
    let index = slot - 1;
    AGENT_COLOR_TOKENS[index.rem_euclid(len) as usize]
}

/// Thinking levels with a border color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingBorderLevel {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
}

/// Where a theme came from, for callers that report it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeOptions {
    pub name: Option<String>,
    pub source_path: Option<PathBuf>,
}

/// `hasLightSurfaces`: whether the theme paints on paper, judged from
/// `userMessageBg` (then `selectedBg`).
fn has_light_surfaces(bg: &[(String, RawColor)]) -> bool {
    for key in ["userMessageBg", "selectedBg"] {
        let Some((_, candidate)) = bg.iter().find(|(k, _)| k == key) else {
            continue;
        };
        if candidate.is_default() {
            continue;
        }
        if let Some(luminance) = relative_luminance(candidate) {
            return luminance > 0.5;
        }
    }
    false
}

/// A theme resolved for one color mode.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: Option<String>,
    pub source_path: Option<PathBuf>,
    fg_colors: HashMap<String, String>,
    bg_colors: HashMap<String, String>,
    /// Foreground tokens usable as a chip fill: background ANSI plus ink.
    fill_colors: HashMap<String, String>,
    mode: ColorMode,
}

impl Theme {
    /// `new Theme(fgColors, bgColors, mode, options)`. Fails on a color value
    /// that cannot be encoded (an invalid hex).
    pub fn new(
        fg_colors: Vec<(String, RawColor)>,
        bg_colors: Vec<(String, RawColor)>,
        mode: ColorMode,
        options: ThemeOptions,
    ) -> Result<Self, String> {
        // Fallbacks apply to raw values: a fill is derived from the color itself.
        let mut raw = fg_colors;
        let get = |raw: &Vec<(String, RawColor)>, key: &str| {
            raw.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
        };
        if let Some(accent) = get(&raw, "accent") {
            for token in AGENT_COLOR_TOKENS {
                if get(&raw, token).is_none() {
                    raw.push((token.to_string(), accent.clone()));
                }
            }
        }
        if let Some(dim) = get(&raw, "dim") {
            if get(&raw, "halftone").is_none() {
                raw.push(("halftone".to_string(), dim));
            }
        }

        let light_backdrop = has_light_surfaces(&bg_colors);
        let mut fg = HashMap::new();
        let mut fills = HashMap::new();
        for (key, value) in &raw {
            fg.insert(key.clone(), fg_ansi(value, mode)?);
            // "" means the terminal's own foreground: nothing to fill with.
            let fill = if value.is_default() {
                value.clone()
            } else {
                chip_fill(value, light_backdrop, mode)
            };
            let ink = if fill.is_default() {
                None
            } else {
                fill_ink(&rendered_color(&fill, mode))
            };
            if let Some((ink, _)) = ink {
                fills.insert(
                    key.clone(),
                    bg_ansi(&fill, mode)? + &fg_ansi(&RawColor::hex(ink), mode)?,
                );
            }
        }
        let mut bg = HashMap::new();
        for (key, value) in &bg_colors {
            bg.insert(key.clone(), bg_ansi(value, mode)?);
        }
        Ok(Self {
            name: options.name,
            source_path: options.source_path,
            fg_colors: fg,
            bg_colors: bg,
            fill_colors: fills,
            mode,
        })
    }

    /// `fg`: `text` in a foreground token, resetting only the foreground.
    /// Panics on an unknown token, as hoocode throws; check [`Theme::has`]
    /// first for optional tokens.
    pub fn fg(&self, color: &str, text: &str) -> String {
        format!("{}{text}\x1b[39m", self.get_fg_ansi(color))
    }

    /// `bg`: `text` on a background token, resetting only the background.
    pub fn bg(&self, color: &str, text: &str) -> String {
        format!("{}{text}\x1b[49m", self.get_bg_ansi(color))
    }

    /// `fill`: `text` as a filled chip in a foreground token, with legible
    /// ink; colored text when the token is not fillable.
    pub fn fill(&self, color: &str, text: &str) -> String {
        match self.fill_colors.get(color) {
            Some(ansi) => format!("{ansi}{text}\x1b[39m\x1b[49m"),
            None => self.fg(color, text),
        }
    }

    /// `canFill`.
    pub fn can_fill(&self, color: &str) -> bool {
        self.fill_colors.contains_key(color)
    }

    /// `has`: whether the theme defines a foreground token.
    pub fn has(&self, color: &str) -> bool {
        self.fg_colors.contains_key(color)
    }

    /// `hasBg`.
    pub fn has_bg(&self, color: &str) -> bool {
        self.bg_colors.contains_key(color)
    }

    pub fn bold(&self, text: &str) -> String {
        chalk::bold(text)
    }

    /// Terminal-native blink (SGR 5).
    pub fn blink(&self, text: &str) -> String {
        format!("\x1b[5m{text}\x1b[25m")
    }

    pub fn italic(&self, text: &str) -> String {
        chalk::italic(text)
    }

    pub fn underline(&self, text: &str) -> String {
        chalk::underline(text)
    }

    pub fn inverse(&self, text: &str) -> String {
        chalk::inverse(text)
    }

    pub fn strikethrough(&self, text: &str) -> String {
        chalk::strikethrough(text)
    }

    /// `getFgAnsi`. Panics on an unknown token.
    pub fn get_fg_ansi(&self, color: &str) -> &str {
        self.fg_colors
            .get(color)
            .unwrap_or_else(|| panic!("Unknown theme color: {color}"))
    }

    /// `getBgAnsi`. Panics on an unknown token.
    pub fn get_bg_ansi(&self, color: &str) -> &str {
        self.bg_colors
            .get(color)
            .unwrap_or_else(|| panic!("Unknown theme background color: {color}"))
    }

    /// `getColorMode`.
    pub fn get_color_mode(&self) -> ColorMode {
        self.mode
    }

    /// `getThinkingBorderColor`: the token a thinking level's border uses.
    pub fn thinking_border_token(level: ThinkingBorderLevel) -> &'static str {
        match level {
            ThinkingBorderLevel::Off => "thinkingOff",
            ThinkingBorderLevel::Minimal => "thinkingMinimal",
            ThinkingBorderLevel::Low => "thinkingLow",
            ThinkingBorderLevel::Medium => "thinkingMedium",
            ThinkingBorderLevel::High => "thinkingHigh",
            ThinkingBorderLevel::Xhigh => "thinkingXhigh",
        }
    }

    /// `getThinkingBorderColor(level)(text)`.
    pub fn thinking_border(&self, level: ThinkingBorderLevel, text: &str) -> String {
        self.fg(Self::thinking_border_token(level), text)
    }

    /// `getBashModeBorderColor()(text)`.
    pub fn bash_mode_border(&self, text: &str) -> String {
        self.fg("bashMode", text)
    }
}
