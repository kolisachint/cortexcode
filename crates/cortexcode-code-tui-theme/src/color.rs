//! Color utilities from `theme.ts`: mode detection, hex/256/HSL conversion,
//! ANSI encoding, WCAG luminance and contrast, and the chip-fill lift.
//!
//! Arithmetic follows the JavaScript it was ported from (`Math.round` is
//! round-half-up, float accumulation in the lift walk is kept as written), so
//! the colors a theme emits match hoocode's byte for byte.

use std::fmt;

/// `ColorMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    Truecolor,
    Color256,
}

impl ColorMode {
    /// `"truecolor"` / `"256color"`.
    pub fn as_str(self) -> &'static str {
        match self {
            ColorMode::Truecolor => "truecolor",
            ColorMode::Color256 => "256color",
        }
    }
}

/// A resolved color value: a hex string (`"#rrggbb"`), `""` for the
/// terminal's default, or a 256-color index (`string | number` in hoocode).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawColor {
    Str(String),
    Index(u8),
}

impl RawColor {
    pub fn hex(s: &str) -> Self {
        RawColor::Str(s.to_string())
    }

    pub fn is_default(&self) -> bool {
        matches!(self, RawColor::Str(s) if s.is_empty())
    }
}

impl fmt::Display for RawColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RawColor::Str(s) => f.write_str(s),
            RawColor::Index(n) => write!(f, "{n}"),
        }
    }
}

/// `detectColorMode` over an environment lookup.
pub fn detect_color_mode_with(env: impl Fn(&str) -> Option<String>) -> ColorMode {
    let colorterm = env("COLORTERM");
    if matches!(colorterm.as_deref(), Some("truecolor") | Some("24bit")) {
        return ColorMode::Truecolor;
    }
    // Windows Terminal supports truecolor.
    if env("WT_SESSION").is_some_and(|v| !v.is_empty()) {
        return ColorMode::Truecolor;
    }
    let term = env("TERM").unwrap_or_default();
    // Fall back to 256color for truly limited terminals.
    if term == "dumb" || term.is_empty() || term == "linux" {
        return ColorMode::Color256;
    }
    // Terminal.app doesn't support truecolor.
    if env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal") {
        return ColorMode::Color256;
    }
    // GNU screen doesn't unless opted in via COLORTERM=truecolor.
    if term == "screen" || term.starts_with("screen-") || term.starts_with("screen.") {
        return ColorMode::Color256;
    }
    ColorMode::Truecolor
}

/// `detectColorMode` from the process environment.
pub fn detect_color_mode() -> ColorMode {
    detect_color_mode_with(|k| std::env::var(k).ok())
}

/// JS `parseInt(s, 16)`: leading whitespace, an optional sign, an optional
/// `0x`, then the longest run of hex digits; `None` for NaN.
fn parse_int16(s: &str) -> Option<i64> {
    let s = s.trim_start();
    let (neg, s) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let s = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .unwrap_or(s);
    let digits: String = s.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    if digits.is_empty() {
        return None;
    }
    let v = i64::from_str_radix(&digits, 16).ok()?;
    Some(if neg { -v } else { v })
}

/// `hexToRgb`.
pub fn hex_to_rgb(hex: &str) -> Result<(i64, i64, i64), String> {
    let cleaned = hex.replacen('#', "", 1);
    let units: Vec<u16> = cleaned.encode_utf16().collect();
    if units.len() != 6 {
        return Err(format!("Invalid hex color: {hex}"));
    }
    let part = |a: usize, b: usize| String::from_utf16_lossy(&units[a..b]);
    let (r, g, b) = (
        parse_int16(&part(0, 2)),
        parse_int16(&part(2, 4)),
        parse_int16(&part(4, 6)),
    );
    match (r, g, b) {
        (Some(r), Some(g), Some(b)) => Ok((r, g, b)),
        _ => Err(format!("Invalid hex color: {hex}")),
    }
}

/// JS `Math.round`.
pub(crate) fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// The 6x6x6 color cube channel values.
const CUBE_VALUES: [i64; 6] = [0, 95, 135, 175, 215, 255];

fn gray_values() -> [i64; 24] {
    std::array::from_fn(|i| 8 + i as i64 * 10)
}

fn find_closest_index(value: f64, values: &[i64]) -> usize {
    let mut min_dist = f64::INFINITY;
    let mut min_idx = 0;
    for (i, &v) in values.iter().enumerate() {
        let dist = (value - v as f64).abs();
        if dist < min_dist {
            min_dist = dist;
            min_idx = i;
        }
    }
    min_idx
}

fn color_distance(r1: f64, g1: f64, b1: f64, r2: f64, g2: f64, b2: f64) -> f64 {
    // Weighted Euclidean distance (the eye is more sensitive to green).
    let (dr, dg, db) = (r1 - r2, g1 - g2, b1 - b2);
    dr * dr * 0.299 + dg * dg * 0.587 + db * db * 0.114
}

/// `rgbTo256`.
pub fn rgb_to_256(r: i64, g: i64, b: i64) -> i64 {
    let (rf, gf, bf) = (r as f64, g as f64, b as f64);
    let r_idx = find_closest_index(rf, &CUBE_VALUES);
    let g_idx = find_closest_index(gf, &CUBE_VALUES);
    let b_idx = find_closest_index(bf, &CUBE_VALUES);
    let (cube_r, cube_g, cube_b) = (CUBE_VALUES[r_idx], CUBE_VALUES[g_idx], CUBE_VALUES[b_idx]);
    let cube_index = 16 + 36 * r_idx as i64 + 6 * g_idx as i64 + b_idx as i64;
    let cube_dist = color_distance(rf, gf, bf, cube_r as f64, cube_g as f64, cube_b as f64);

    let grays = gray_values();
    let gray = js_round(0.299 * rf + 0.587 * gf + 0.114 * bf);
    let gray_idx = find_closest_index(gray, &grays);
    let gray_value = grays[gray_idx] as f64;
    let gray_index = 232 + gray_idx as i64;
    let gray_dist = color_distance(rf, gf, bf, gray_value, gray_value, gray_value);

    // Only consider grayscale if the color is nearly neutral and gray is closer.
    let spread = r.max(g).max(b) - r.min(g).min(b);
    if spread < 10 && gray_dist < cube_dist {
        return gray_index;
    }
    cube_index
}

/// `hexTo256`.
pub fn hex_to_256(hex: &str) -> Result<i64, String> {
    let (r, g, b) = hex_to_rgb(hex)?;
    Ok(rgb_to_256(r, g, b))
}

fn ansi(color: &RawColor, mode: ColorMode, base: u8, default: &str) -> Result<String, String> {
    match color {
        RawColor::Str(s) if s.is_empty() => Ok(default.to_string()),
        RawColor::Index(n) => Ok(format!("\x1b[{base};5;{n}m")),
        RawColor::Str(s) if s.starts_with('#') => match mode {
            ColorMode::Truecolor => {
                let (r, g, b) = hex_to_rgb(s)?;
                Ok(format!("\x1b[{base};2;{r};{g};{b}m"))
            }
            ColorMode::Color256 => Ok(format!("\x1b[{base};5;{}m", hex_to_256(s)?)),
        },
        RawColor::Str(s) => Err(format!("Invalid color value: {s}")),
    }
}

/// `fgAnsi`.
pub fn fg_ansi(color: &RawColor, mode: ColorMode) -> Result<String, String> {
    ansi(color, mode, 38, "\x1b[39m")
}

/// `bgAnsi`.
pub fn bg_ansi(color: &RawColor, mode: ColorMode) -> Result<String, String> {
    ansi(color, mode, 48, "\x1b[49m")
}

/// JS `n.toString(16).padStart(2, "0")`.
fn hex2(n: i64) -> String {
    let s = if n < 0 {
        format!("-{:x}", -n)
    } else {
        format!("{n:x}")
    };
    format!("{s:0>2}")
}

/// `ansi256ToHex`: indices 0-15 approximate common terminal values, 16-231
/// the cube, 232-255 the grayscale ramp.
pub fn ansi256_to_hex(index: i64) -> String {
    const BASIC: [&str; 16] = [
        "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080", "#c0c0c0",
        "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff", "#00ffff", "#ffffff",
    ];
    if (0..16).contains(&index) {
        return BASIC[index as usize].to_string();
    }
    if index < 232 {
        let cube = index - 16;
        let (r, g, b) = (
            cube.div_euclid(36),
            cube.rem_euclid(36) / 6,
            cube.rem_euclid(6),
        );
        let to_hex = |n: i64| hex2(if n == 0 { 0 } else { 55 + n * 40 });
        return format!("#{}{}{}", to_hex(r), to_hex(g), to_hex(b));
    }
    let gray = hex2(8 + (index - 232) * 10);
    format!("#{gray}{gray}{gray}")
}

/// A value as hex: a 256-color index converted, a string as is.
fn as_hex(color: &RawColor) -> String {
    match color {
        RawColor::Index(n) => ansi256_to_hex(*n as i64),
        RawColor::Str(s) => s.clone(),
    }
}

/// `relativeLuminance`: WCAG relative luminance (0-1), or `None` when the
/// color cannot be parsed.
pub fn relative_luminance(color: &RawColor) -> Option<f64> {
    let hex = as_hex(color);
    if !hex.starts_with('#') {
        return None;
    }
    let (r, g, b) = hex_to_rgb(&hex).ok()?;
    let to_linear = |c: i64| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Some(0.2126 * to_linear(r) + 0.7152 * to_linear(g) + 0.0722 * to_linear(b))
}

/// `contrastRatio`: WCAG contrast, or `None` if either side is unparseable.
pub fn contrast_ratio(a: &RawColor, b: &RawColor) -> Option<f64> {
    let (first, second) = (relative_luminance(a)?, relative_luminance(b)?);
    Some((first.max(second) + 0.05) / (first.min(second) + 0.05))
}

/// The two inks a chip can be written in.
pub const CHIP_INK_DARK: &str = "#0b0b0f";
pub const CHIP_INK_LIGHT: &str = "#ffffff";

/// `fillInk`: whichever ink measures better against the fill, and its ratio.
pub fn fill_ink(color: &RawColor) -> Option<(&'static str, f64)> {
    let dark = contrast_ratio(color, &RawColor::hex(CHIP_INK_DARK))?;
    let light = contrast_ratio(color, &RawColor::hex(CHIP_INK_LIGHT))?;
    Some(if dark >= light {
        (CHIP_INK_DARK, dark)
    } else {
        (CHIP_INK_LIGHT, light)
    })
}

/// How light a chip fill has to be before its hue reads as that hue.
const MIN_CHIP_FILL_LIGHTNESS: f64 = 0.5;
/// How well the name on a chip has to read against its fill.
const MIN_CHIP_INK_CONTRAST: f64 = 5.5;
/// The same bar, as a 256-color terminal is able to keep it.
const MIN_CHIP_INK_CONTRAST_QUANTIZED: f64 = 4.5;
/// Lightness past which lifting stops, so a hue is never bleached to paper.
const MAX_CHIP_FILL_LIGHTNESS: f64 = 0.88;
/// Saturation a lifted fill is held under.
const MAX_CHIP_FILL_SATURATION: f64 = 0.72;
/// Step the lift walks in.
const CHIP_FILL_LIGHTNESS_STEP: f64 = 0.02;

/// The magenta hue window a chip keeps white ink for (see `chip_fill`).
pub const MAGENTA_HUE_MIN: f64 = 305.0;
pub const MAGENTA_HUE_MAX: f64 = 355.0;
pub const MIN_MAGENTA_SATURATION: f64 = 0.25;
/// Darkest a magenta fill is deepened to in search of white-clearing ink.
const MIN_MAGENTA_FILL_LIGHTNESS: f64 = 0.3;

/// HSL (0-360 / 0-1 / 0-1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsl {
    pub h: f64,
    pub s: f64,
    pub l: f64,
}

/// `hexToHsl`.
pub fn hex_to_hsl(hex: &str) -> Result<Hsl, String> {
    let (r, g, b) = hex_to_rgb(hex)?;
    let (red, green, blue) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = red.max(green).max(blue);
    let min = red.min(green).min(blue);
    let delta = max - min;
    let l = (max + min) / 2.0;
    if delta == 0.0 {
        return Ok(Hsl { h: 0.0, s: 0.0, l });
    }
    let raw = if max == red {
        (green - blue) / delta + if green < blue { 6.0 } else { 0.0 }
    } else if max == green {
        (blue - red) / delta + 2.0
    } else {
        (red - green) / delta + 4.0
    };
    Ok(Hsl {
        h: raw * 60.0,
        s: delta / (1.0 - (2.0 * l - 1.0).abs()),
        l,
    })
}

/// JS `%` (remainder with the dividend's sign).
fn js_rem(a: f64, b: f64) -> f64 {
    a % b
}

/// `hslToHex`.
pub fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let sector = js_rem((js_rem(js_rem(h, 360.0) + 360.0, 360.0)) / 60.0, 6.0);
    let second = chroma * (1.0 - (js_rem(sector, 2.0) - 1.0).abs());
    let (r, g, b) = if sector < 1.0 {
        (chroma, second, 0.0)
    } else if sector < 2.0 {
        (second, chroma, 0.0)
    } else if sector < 3.0 {
        (0.0, chroma, second)
    } else if sector < 4.0 {
        (0.0, second, chroma)
    } else if sector < 5.0 {
        (second, 0.0, chroma)
    } else {
        (chroma, 0.0, second)
    };
    let offset = l - chroma / 2.0;
    let ch = |c: f64| hex2(js_round((c + offset) * 255.0) as i64);
    format!("#{}{}{}", ch(r), ch(g), ch(b))
}

/// `renderedColor`: what a color looks like once the terminal has painted it
/// (rounded to the cube on a 256-color terminal).
pub fn rendered_color(color: &RawColor, mode: ColorMode) -> RawColor {
    if mode != ColorMode::Color256 {
        return color.clone();
    }
    match color {
        RawColor::Str(s) if s.starts_with('#') => match hex_to_256(s) {
            Ok(index) => RawColor::Str(ansi256_to_hex(index)),
            Err(_) => color.clone(),
        },
        _ => color.clone(),
    }
}

/// `magentaHsl`: the HSL of a palette entry known to be a saturated magenta.
fn magenta_hsl(color: &RawColor) -> Option<Hsl> {
    let hex = as_hex(color);
    if !hex.starts_with('#') {
        return None;
    }
    let hsl = hex_to_hsl(&hex).ok()?;
    if hsl.s <= MIN_MAGENTA_SATURATION {
        return None;
    }
    let hue = js_rem(js_rem(hsl.h, 360.0) + 360.0, 360.0);
    if !(MAGENTA_HUE_MIN..=MAGENTA_HUE_MAX).contains(&hue) {
        return None;
    }
    Some(Hsl {
        h: hue,
        s: hsl.s,
        l: hsl.l,
    })
}

/// `clearsWithWhite`.
fn clears_with_white(candidate: &RawColor, mode: ColorMode) -> bool {
    let white = RawColor::hex(CHIP_INK_LIGHT);
    let exact = contrast_ratio(candidate, &white).unwrap_or(0.0);
    let rounded = contrast_ratio(&rendered_color(candidate, mode), &white).unwrap_or(0.0);
    exact >= MIN_CHIP_INK_CONTRAST && rounded >= MIN_CHIP_INK_CONTRAST_QUANTIZED
}

/// `chipFill`: the color a chip is actually filled with for a palette entry.
/// Lifts (or, for magenta, deepens) only a fill that cannot carry its ink or,
/// on a light theme, is too dark to name its hue; the hue is always kept.
pub fn chip_fill(color: &RawColor, light_backdrop: bool, mode: ColorMode) -> RawColor {
    if let Some(magenta) = magenta_hsl(color) {
        if clears_with_white(color, mode) {
            return color.clone();
        }
        let mut lightness = magenta.l - CHIP_FILL_LIGHTNESS_STEP;
        while lightness >= MIN_MAGENTA_FILL_LIGHTNESS {
            let candidate = RawColor::Str(hsl_to_hex(
                magenta.h,
                magenta.s.min(MAX_CHIP_FILL_SATURATION),
                lightness,
            ));
            if clears_with_white(&candidate, mode) {
                return candidate;
            }
            lightness -= CHIP_FILL_LIGHTNESS_STEP;
        }
        // No deep variant clears: fall through to the lift.
    }

    let clears = |candidate: &RawColor| {
        fill_ink(candidate).map_or(0.0, |(_, r)| r) >= MIN_CHIP_INK_CONTRAST
            && fill_ink(&rendered_color(candidate, mode)).map_or(0.0, |(_, r)| r)
                >= MIN_CHIP_INK_CONTRAST_QUANTIZED
    };
    if !light_backdrop && clears(color) {
        return color.clone();
    }

    let hex = as_hex(color);
    if !hex.starts_with('#') {
        return color.clone();
    }
    let Ok(hsl) = hex_to_hsl(&hex) else {
        return color.clone();
    };

    let floor = if light_backdrop && hsl.s > 0.0 {
        hsl.l.max(MIN_CHIP_FILL_LIGHTNESS)
    } else {
        hsl.l
    };
    let saturation = if floor > hsl.l {
        hsl.s.min(MAX_CHIP_FILL_SATURATION)
    } else {
        hsl.s
    };

    let mut lightness = floor;
    while lightness <= MAX_CHIP_FILL_LIGHTNESS {
        let candidate = if lightness <= hsl.l && saturation == hsl.s {
            color.clone()
        } else {
            RawColor::Str(hsl_to_hex(hsl.h, saturation, lightness))
        };
        if clears(&candidate) {
            return candidate;
        }
        lightness += CHIP_FILL_LIGHTNESS_STEP;
    }
    RawColor::Str(hsl_to_hex(hsl.h, saturation, MAX_CHIP_FILL_LIGHTNESS))
}
