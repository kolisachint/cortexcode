//! Terminal image rendering for the cortex TUI.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` -> `terminal-image.ts`:
//! terminal capability detection, Kitty/iTerm2 inline image protocol
//! encoding, and lightweight image-dimension sniffing for PNG/JPEG/GIF/WebP.

mod dimensions;
mod iterm2;
mod kitty;

pub use dimensions::{
    get_gif_dimensions, get_image_dimensions, get_jpeg_dimensions, get_png_dimensions,
    get_webp_dimensions, ImageDimensions,
};
pub use iterm2::{encode_iterm2, ITerm2EncodeOptions};
pub use kitty::{delete_all_kitty_images, delete_kitty_image, encode_kitty, KittyEncodeOptions};

use once_cell::sync::Lazy;
use rand::Rng;
use std::env;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProtocol {
    Kitty,
    ITerm2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalCapabilities {
    pub images: Option<ImageProtocol>,
    pub true_color: bool,
    pub hyperlinks: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellDimensions {
    pub width_px: u32,
    pub height_px: u32,
}

impl Default for CellDimensions {
    fn default() -> Self {
        Self {
            width_px: 9,
            height_px: 18,
        }
    }
}

static CACHED_CAPABILITIES: Lazy<Mutex<Option<TerminalCapabilities>>> =
    Lazy::new(|| Mutex::new(None));
static CELL_DIMENSIONS: Lazy<Mutex<CellDimensions>> =
    Lazy::new(|| Mutex::new(CellDimensions::default()));

pub fn get_cell_dimensions() -> CellDimensions {
    *CELL_DIMENSIONS.lock().unwrap()
}

pub fn set_cell_dimensions(dims: CellDimensions) {
    *CELL_DIMENSIONS.lock().unwrap() = dims;
}

fn env_lower(key: &str) -> String {
    env::var(key).unwrap_or_default().to_lowercase()
}

pub fn detect_capabilities() -> TerminalCapabilities {
    let term_program = env_lower("TERM_PROGRAM");
    let term = env_lower("TERM");
    let color_term = env_lower("COLORTERM");

    let in_tmux_or_screen =
        env::var("TMUX").is_ok() || term.starts_with("tmux") || term.starts_with("screen");
    if in_tmux_or_screen {
        let true_color = color_term == "truecolor" || color_term == "24bit";
        return TerminalCapabilities {
            images: None,
            true_color,
            hyperlinks: false,
        };
    }

    if env::var("KITTY_WINDOW_ID").is_ok() || term_program == "kitty" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if term_program == "ghostty"
        || term.contains("ghostty")
        || env::var("GHOSTTY_RESOURCES_DIR").is_ok()
    {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if env::var("WEZTERM_PANE").is_ok() || term_program == "wezterm" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        };
    }

    if env::var("ITERM_SESSION_ID").is_ok() || term_program == "iterm.app" {
        return TerminalCapabilities {
            images: Some(ImageProtocol::ITerm2),
            true_color: true,
            hyperlinks: true,
        };
    }

    if term_program == "vscode" {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: true,
        };
    }

    if term_program == "alacritty" {
        return TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: true,
        };
    }

    let true_color = color_term == "truecolor" || color_term == "24bit";
    TerminalCapabilities {
        images: None,
        true_color,
        hyperlinks: false,
    }
}

pub fn get_capabilities() -> TerminalCapabilities {
    let mut cached = CACHED_CAPABILITIES.lock().unwrap();
    if cached.is_none() {
        *cached = Some(detect_capabilities());
    }
    cached.unwrap()
}

pub fn reset_capabilities_cache() {
    *CACHED_CAPABILITIES.lock().unwrap() = None;
}

/// Override the cached capabilities. Useful in tests to exercise both code paths.
pub fn set_capabilities(caps: TerminalCapabilities) {
    *CACHED_CAPABILITIES.lock().unwrap() = Some(caps);
}

const KITTY_PREFIX: &str = "\x1b_G";
const ITERM2_PREFIX: &str = "\x1b]1337;File=";

pub fn is_image_line(line: &str) -> bool {
    line.starts_with(KITTY_PREFIX)
        || line.starts_with(ITERM2_PREFIX)
        || line.contains(KITTY_PREFIX)
        || line.contains(ITERM2_PREFIX)
}

/// Generate a random image ID for the Kitty graphics protocol, in range [1, 0xffffffff].
pub fn allocate_image_id() -> u32 {
    rand::thread_rng().gen_range(0..0xffff_fffe) + 1
}

pub fn calculate_image_rows(
    image_dimensions: ImageDimensions,
    target_width_cells: u32,
    cell_dimensions: CellDimensions,
) -> u32 {
    let target_width_px = target_width_cells as f64 * cell_dimensions.width_px as f64;
    let scale = target_width_px / image_dimensions.width_px as f64;
    let scaled_height_px = image_dimensions.height_px as f64 * scale;
    let rows = (scaled_height_px / cell_dimensions.height_px as f64).ceil() as i64;
    rows.max(1) as u32
}

#[derive(Debug, Clone, Default)]
pub struct ImageRenderOptions {
    pub max_width_cells: Option<u32>,
    pub max_height_cells: Option<u32>,
    pub preserve_aspect_ratio: Option<bool>,
    /// Kitty image ID. If provided, reuses/replaces existing image with this ID.
    pub image_id: Option<u32>,
    /// Whether Kitty should apply its default cursor movement after placement.
    pub move_cursor: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct RenderedImage {
    pub sequence: String,
    pub rows: u32,
    pub image_id: Option<u32>,
}

pub fn render_image(
    base64_data: &str,
    image_dimensions: ImageDimensions,
    options: &ImageRenderOptions,
) -> Option<RenderedImage> {
    let caps = get_capabilities();
    let protocol = caps.images?;

    let max_width = options.max_width_cells.unwrap_or(80);
    let rows = calculate_image_rows(image_dimensions, max_width, get_cell_dimensions());

    match protocol {
        ImageProtocol::Kitty => {
            let sequence = encode_kitty(
                base64_data,
                &KittyEncodeOptions {
                    columns: Some(max_width),
                    rows: Some(rows),
                    image_id: options.image_id,
                    move_cursor: options.move_cursor,
                },
            );
            Some(RenderedImage {
                sequence,
                rows,
                image_id: options.image_id,
            })
        }
        ImageProtocol::ITerm2 => {
            let sequence = encode_iterm2(
                base64_data,
                &ITerm2EncodeOptions {
                    width: Some(max_width.to_string()),
                    height: Some("auto".to_string()),
                    preserve_aspect_ratio: Some(options.preserve_aspect_ratio.unwrap_or(true)),
                    ..Default::default()
                },
            );
            Some(RenderedImage {
                sequence,
                rows,
                image_id: None,
            })
        }
    }
}

/// Wrap text in an OSC 8 hyperlink sequence.
pub fn hyperlink(text: &str, url: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

pub fn image_fallback(
    mime_type: &str,
    dimensions: Option<ImageDimensions>,
    filename: Option<&str>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = filename {
        parts.push(name.to_string());
    }
    parts.push(format!("[{mime_type}]"));
    if let Some(d) = dimensions {
        parts.push(format!("{}x{}", d.width_px, d.height_px));
    }
    format!("[Image: {}]", parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Serializes tests that touch process env vars / the global capabilities cache.
    static ENV_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    const ENV_KEYS: &[&str] = &[
        "TERM",
        "TERM_PROGRAM",
        "COLORTERM",
        "TMUX",
        "KITTY_WINDOW_ID",
        "GHOSTTY_RESOURCES_DIR",
        "WEZTERM_PANE",
        "ITERM_SESSION_ID",
    ];

    fn with_env<F: FnOnce()>(overrides: &[(&str, &str)], f: F) {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let saved: Vec<(&str, Option<String>)> =
            ENV_KEYS.iter().map(|k| (*k, env::var(k).ok())).collect();
        for k in ENV_KEYS {
            env::remove_var(k);
        }
        for (k, v) in overrides {
            env::set_var(k, v);
        }
        f();
        for (k, v) in saved {
            match v {
                Some(val) => env::set_var(k, val),
                None => env::remove_var(k),
            }
        }
    }

    #[test]
    fn is_image_line_detects_iterm2_at_start() {
        let line = "\x1b]1337;File=size=100,100;inline=1:base64encodeddata==\x07";
        assert!(is_image_line(line));
    }

    #[test]
    fn is_image_line_detects_iterm2_with_surrounding_text() {
        let line = "Some text \x1b]1337;File=inline=1:base64data==\x07 more text";
        assert!(is_image_line(line));
    }

    #[test]
    fn is_image_line_detects_kitty_sequence() {
        let line = "\x1b_Ga=T,f=100,t=f,d=base64data...\x1b\\\x1b_Gm=i=1;\x1b\\";
        assert!(is_image_line(line));
    }

    #[test]
    fn is_image_line_negative_cases() {
        assert!(!is_image_line("This is just a regular text line"));
        assert!(!is_image_line("\x1b[31mRed text\x1b[0m"));
        assert!(!is_image_line(""));
        assert!(!is_image_line("\n"));
        assert!(!is_image_line("/path/to/File_1337_backup/image.jpg"));
        assert!(!is_image_line(
            "Some text with ]1337;File but missing ESC at start"
        ));
    }

    #[test]
    fn is_image_line_handles_very_long_lines() {
        let base64_char = "A".repeat(100);
        let image_sequence = "\x1b]1337;File=size=800,600;inline=1:";
        let long_line = format!(
            "Text prefix {image_sequence}{} suffix",
            base64_char.repeat(3000)
        );
        assert!(long_line.len() > 300_000);
        assert!(is_image_line(&long_line));
    }

    #[test]
    fn detect_capabilities_defaults_to_no_hyperlinks() {
        with_env(&[], || {
            let caps = detect_capabilities();
            assert!(!caps.hyperlinks);
            assert_eq!(caps.images, None);
        });
    }

    #[test]
    fn detect_capabilities_forces_no_hyperlinks_under_tmux() {
        with_env(
            &[
                ("TMUX", "/tmp/tmux-1000/default,1234,0"),
                ("TERM_PROGRAM", "ghostty"),
            ],
            || {
                let caps = detect_capabilities();
                assert!(!caps.hyperlinks);
                assert_eq!(caps.images, None);
            },
        );
    }

    #[test]
    fn detect_capabilities_forces_no_hyperlinks_when_term_starts_with_screen() {
        with_env(&[("TERM", "screen-256color")], || {
            let caps = detect_capabilities();
            assert!(!caps.hyperlinks);
            assert_eq!(caps.images, None);
        });
    }

    #[test]
    fn detect_capabilities_enables_kitty() {
        with_env(&[("KITTY_WINDOW_ID", "1")], || {
            let caps = detect_capabilities();
            assert!(caps.hyperlinks);
            assert_eq!(caps.images, Some(ImageProtocol::Kitty));
        });
    }

    #[test]
    fn detect_capabilities_enables_ghostty() {
        with_env(&[("TERM_PROGRAM", "ghostty")], || {
            let caps = detect_capabilities();
            assert!(caps.hyperlinks);
            assert_eq!(caps.images, Some(ImageProtocol::Kitty));
        });
    }

    #[test]
    fn detect_capabilities_enables_iterm2() {
        with_env(&[("TERM_PROGRAM", "iterm.app")], || {
            let caps = detect_capabilities();
            assert!(caps.hyperlinks);
            assert_eq!(caps.images, Some(ImageProtocol::ITerm2));
        });
    }

    #[test]
    fn detect_capabilities_vscode_has_no_images() {
        with_env(&[("TERM_PROGRAM", "vscode")], || {
            let caps = detect_capabilities();
            assert!(caps.hyperlinks);
            assert_eq!(caps.images, None);
        });
    }

    #[test]
    fn render_image_kitty_default_moves_cursor() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        });
        set_cell_dimensions(CellDimensions {
            width_px: 10,
            height_px: 10,
        });
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 20,
                height_px: 20,
            },
            &ImageRenderOptions {
                max_width_cells: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!result.sequence.contains(",C=1,"));
        assert_eq!(result.rows, 2);
        reset_capabilities_cache();
        set_cell_dimensions(CellDimensions::default());
    }

    #[test]
    fn render_image_kitty_can_opt_out_of_cursor_move() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        set_capabilities(TerminalCapabilities {
            images: Some(ImageProtocol::Kitty),
            true_color: true,
            hyperlinks: true,
        });
        set_cell_dimensions(CellDimensions {
            width_px: 10,
            height_px: 10,
        });
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 20,
                height_px: 20,
            },
            &ImageRenderOptions {
                max_width_cells: Some(2),
                move_cursor: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(result.sequence.contains(",C=1,"));
        assert_eq!(result.rows, 2);
        reset_capabilities_cache();
        set_cell_dimensions(CellDimensions::default());
    }

    #[test]
    fn render_image_returns_none_without_image_support() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        set_capabilities(TerminalCapabilities {
            images: None,
            true_color: true,
            hyperlinks: true,
        });
        let result = render_image(
            "AAAA",
            ImageDimensions {
                width_px: 20,
                height_px: 20,
            },
            &ImageRenderOptions::default(),
        );
        assert!(result.is_none());
        reset_capabilities_cache();
    }

    #[test]
    fn hyperlink_wraps_text_in_osc8() {
        let result = hyperlink("click me", "https://example.com");
        assert_eq!(
            result,
            "\x1b]8;;https://example.com\x1b\\click me\x1b]8;;\x1b\\"
        );
    }

    #[test]
    fn hyperlink_works_with_empty_text() {
        let result = hyperlink("", "https://example.com");
        assert_eq!(result, "\x1b]8;;https://example.com\x1b\\\x1b]8;;\x1b\\");
    }

    #[test]
    fn image_fallback_formats_parts() {
        let result = image_fallback(
            "image/png",
            Some(ImageDimensions {
                width_px: 100,
                height_px: 50,
            }),
            Some("cat.png"),
        );
        assert_eq!(result, "[Image: cat.png [image/png] 100x50]");
    }

    #[test]
    fn allocate_image_id_is_in_valid_range() {
        for _ in 0..100 {
            let id = allocate_image_id();
            assert!(id >= 1);
        }
    }
}
