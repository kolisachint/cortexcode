//! Color themes for the interactive mode: port of hoocode
//! `modes/interactive/theme/theme.ts` and its bundled JSON themes.
//!
//! - [`color`]: color-mode detection and the color math (256-color
//!   quantization, WCAG contrast, chip fills).
//! - [`schema`]: the theme JSON schema and validation.
//! - [`theme`]: [`Theme`], tokens resolved to ANSI.
//! - [`registry`]: loading (built-in, custom, registered, retired names), the
//!   process-wide current theme, the custom-theme watcher, export colors.
//! - [`tui`]: theme hooks for library components, highlighting, paper sheets.

pub mod chalk;
pub mod color;
pub mod registry;
pub mod schema;
pub mod theme;
pub mod tui;

pub use color::{
    ansi256_to_hex, contrast_ratio, detect_color_mode, detect_color_mode_with, hex_to_256,
    hex_to_rgb, relative_luminance, ColorMode, RawColor, MAGENTA_HUE_MAX, MAGENTA_HUE_MIN,
    MIN_MAGENTA_SATURATION,
};
pub use registry::{
    builtin_theme_source, create_theme, current_theme_name, detect_terminal_background_with,
    get_available_themes, get_available_themes_with_paths, get_resolved_theme_colors,
    get_theme_by_name, get_theme_description, get_theme_export_colors, init_theme,
    load_theme_from_path, load_theme_json, on_theme_change, resolve_theme_name,
    set_registered_themes, set_theme, set_theme_instance, stop_theme_watcher, successor_theme_for,
    theme, try_theme, ThemeExportColors, ThemeInfo, THEME_SCHEMA_JSON,
};
pub use schema::{parse_theme_json, parse_theme_json_content, ThemeJson, REQUIRED_COLOR_TOKENS};
pub use theme::{
    agent_color_for, session_color_token, Theme, ThemeOptions, ThinkingBorderLevel,
    AGENT_COLOR_TOKENS, THEME_BGS, THEME_COLORS,
};
pub use tui::{
    apply_block_fill, apply_paper_sheet, get_editor_theme, get_language_from_path,
    get_markdown_theme, get_paper_shadow_fn, get_select_list_theme, get_settings_list_theme,
    highlight_code, message_label, paint_selected_row, select_gutter, set_code_highlighter,
    style_input, BlockFill, CliHighlight, CliHighlightTheme, CodeHighlighter,
    CLI_HIGHLIGHT_CLASSES, PAPER_INSET, SELECT_CURSOR,
};
