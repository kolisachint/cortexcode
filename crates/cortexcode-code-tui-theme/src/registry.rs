//! Theme loading and the process-wide current theme: built-ins (embedded),
//! the user's `themes/` directory, extension-registered themes, retired
//! names, the custom-theme file watcher, and the export-color helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use crate::color::{ansi256_to_hex, detect_color_mode, relative_luminance, ColorMode, RawColor};
use crate::schema::{parse_theme_json_content, ColorValue, ThemeJson};
use crate::theme::{Theme, ThemeOptions, AGENT_COLOR_TOKENS, THEME_BGS};

/// The bundled theme files, by file name (the `readdirSync().sort()` order).
const BUILTIN_THEME_FILES: [(&str, &str); 8] = [
    (
        "colorsafe-dark.json",
        include_str!("../themes/colorsafe-dark.json"),
    ),
    (
        "colorsafe-light.json",
        include_str!("../themes/colorsafe-light.json"),
    ),
    ("dark.json", include_str!("../themes/dark.json")),
    ("light.json", include_str!("../themes/light.json")),
    (
        "solarized-dark.json",
        include_str!("../themes/solarized-dark.json"),
    ),
    (
        "solarized-light.json",
        include_str!("../themes/solarized-light.json"),
    ),
    (
        "vox-cutout-dark.json",
        include_str!("../themes/vox-cutout-dark.json"),
    ),
    (
        "vox-cutout-light.json",
        include_str!("../themes/vox-cutout-light.json"),
    ),
];

/// The bundled JSON schema for theme files (`theme-schema.json`).
pub const THEME_SCHEMA_JSON: &str = include_str!("../themes/theme-schema.json");

/// The bundled theme file text for `name`, if it is a built-in.
pub fn builtin_theme_source(name: &str) -> Option<&'static str> {
    BUILTIN_THEME_FILES
        .iter()
        .find(|(file, _)| file.strip_suffix(".json") == Some(name))
        .map(|(_, content)| *content)
}

fn builtin_themes() -> &'static BTreeMap<String, ThemeJson> {
    static BUILTINS: OnceLock<BTreeMap<String, ThemeJson>> = OnceLock::new();
    BUILTINS.get_or_init(|| {
        BUILTIN_THEME_FILES
            .iter()
            .map(|(file, content)| {
                let name = file.trim_end_matches(".json").to_string();
                let json = parse_theme_json_content(file, content)
                    .unwrap_or_else(|e| panic!("bundled theme {file} is invalid: {e}"));
                (name, json)
            })
            .collect()
    })
}

/// True for themes shipped with the package.
fn is_builtin_theme(name: &str) -> bool {
    builtin_themes().contains_key(name)
}

/// Built-ins that were retired, and what each one resolves to now.
const RETIRED_THEMES: [(&str, &str); 6] = [
    ("high-contrast-dark", "colorsafe-dark"),
    ("high-contrast-light", "colorsafe-light"),
    ("warm-dark", "colorsafe-dark"),
    ("warm-light", "colorsafe-light"),
    ("vox-dark", "vox-cutout-dark"),
    ("vox-light", "vox-cutout-light"),
];

/// `successorThemeFor`: the theme a retired name now stands for.
pub fn successor_theme_for(name: &str) -> Option<&'static str> {
    RETIRED_THEMES
        .iter()
        .find(|(retired, _)| *retired == name)
        .map(|(_, heir)| *heir)
        .filter(|heir| is_builtin_theme(heir))
}

/// `resolveThemeName`: itself if anything defines it, its successor if
/// retired, else itself.
pub fn resolve_theme_name(name: &str) -> String {
    if get_available_themes().iter().any(|t| t == name) {
        return name.to_string();
    }
    successor_theme_for(name).unwrap_or(name).to_string()
}

fn custom_themes_dir() -> PathBuf {
    cortexcode_code_paths::custom_themes_dir()
}

fn json_files(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|f| f.ends_with(".json"))
        .collect();
    names.sort();
    names
}

/// `getAvailableThemes`: built-ins, custom files and registered themes,
/// sorted.
pub fn get_available_themes() -> Vec<String> {
    let mut themes: BTreeSet<String> = builtin_themes().keys().cloned().collect();
    for file in json_files(&custom_themes_dir()) {
        themes.insert(file.trim_end_matches(".json").to_string());
    }
    for (name, _) in &state().registered {
        themes.insert(name.clone());
    }
    themes.into_iter().collect()
}

/// `getThemeDescription`: the one-line description from a theme file.
pub fn get_theme_description(name: &str) -> Option<String> {
    load_theme_json(name).ok()?.description
}

/// `ThemeInfo`. Built-ins are embedded, so their `path` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeInfo {
    pub name: String,
    pub path: Option<PathBuf>,
}

/// `getAvailableThemesWithPaths`.
pub fn get_available_themes_with_paths() -> Vec<ThemeInfo> {
    let mut result: Vec<ThemeInfo> = builtin_themes()
        .keys()
        .map(|name| ThemeInfo {
            name: name.clone(),
            path: None,
        })
        .collect();
    let dir = custom_themes_dir();
    for file in json_files(&dir) {
        let name = file.trim_end_matches(".json").to_string();
        if !result.iter().any(|t| t.name == name) {
            result.push(ThemeInfo {
                name,
                path: Some(dir.join(&file)),
            });
        }
    }
    for (name, theme) in &state().registered {
        if !result.iter().any(|t| &t.name == name) {
            result.push(ThemeInfo {
                name: name.clone(),
                path: theme.source_path.clone(),
            });
        }
    }
    // `localeCompare`: case-insensitive first.
    result.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
    });
    result
}

fn read_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "ENOENT: no such file or directory, open '{}'",
            path.display()
        ),
        _ => e.to_string(),
    })
}

/// `loadThemeJson`: a theme's JSON by name.
pub fn load_theme_json(name: &str) -> Result<ThemeJson, String> {
    if let Some(json) = builtin_themes().get(name) {
        return Ok(json.clone());
    }
    let registered = registered_theme(name);
    if let Some(theme) = &registered {
        return match &theme.source_path {
            Some(path) => {
                let content = read_file(path)?;
                parse_theme_json_content(&path.display().to_string(), &content)
            }
            None => Err(format!(
                "Theme \"{name}\" does not have a source path for export"
            )),
        };
    }
    let theme_path = custom_themes_dir().join(format!("{name}.json"));
    if !theme_path.exists() {
        // A retired name loads as its successor rather than failing.
        if let Some(successor) = successor_theme_for(name) {
            return Ok(builtin_themes()[successor].clone());
        }
        return Err(format!("Theme not found: {name}"));
    }
    let content = read_file(&theme_path)?;
    parse_theme_json_content(name, &content)
}

/// `resolveVarRefs`.
pub fn resolve_var_refs(
    value: &ColorValue,
    vars: &[(String, ColorValue)],
) -> Result<RawColor, String> {
    let mut visited: Vec<String> = Vec::new();
    let mut current = value.clone();
    loop {
        let name = match &current {
            RawColor::Index(_) => return Ok(current),
            RawColor::Str(s) if s.is_empty() || s.starts_with('#') => return Ok(current),
            RawColor::Str(s) => s.clone(),
        };
        if visited.contains(&name) {
            return Err(format!("Circular variable reference detected: {name}"));
        }
        let Some((_, next)) = vars.iter().find(|(k, _)| *k == name) else {
            return Err(format!("Variable reference not found: {name}"));
        };
        visited.push(name);
        current = next.clone();
    }
}

/// `resolveThemeColors`: every color with its `vars` references resolved.
pub fn resolve_theme_colors(
    colors: &[(String, ColorValue)],
    vars: Option<&[(String, ColorValue)]>,
) -> Result<Vec<(String, RawColor)>, String> {
    let vars = vars.unwrap_or(&[]);
    colors
        .iter()
        .map(|(key, value)| Ok((key.clone(), resolve_var_refs(value, vars)?)))
        .collect()
}

/// `createTheme`.
pub fn create_theme(
    theme_json: &ThemeJson,
    mode: Option<ColorMode>,
    source_path: Option<PathBuf>,
) -> Result<Theme, String> {
    let color_mode = mode.unwrap_or_else(detect_color_mode);
    let resolved = resolve_theme_colors(&theme_json.colors, theme_json.vars.as_deref())?;
    let mut fg: Vec<(String, RawColor)> = Vec::new();
    let mut bg: Vec<(String, RawColor)> = Vec::new();
    for (key, value) in resolved {
        if THEME_BGS.contains(&key.as_str()) {
            bg.push((key, value));
        } else {
            fg.push((key, value));
        }
    }
    // The agent palette is optional; missing entries fall back to accent.
    let accent = fg
        .iter()
        .find(|(k, _)| k == "accent")
        .map(|(_, v)| v.clone());
    for token in AGENT_COLOR_TOKENS {
        if !fg.iter().any(|(k, _)| k == token) {
            if let Some(accent) = &accent {
                fg.push((token.to_string(), accent.clone()));
            }
        }
    }
    // Custom themes predate the warning surface: fall back to customMessageBg.
    if !bg.iter().any(|(k, _)| k == "warningBg") {
        if let Some((_, v)) = bg.iter().find(|(k, _)| k == "customMessageBg").cloned() {
            bg.push(("warningBg".to_string(), v));
        }
    }
    Theme::new(
        fg,
        bg,
        color_mode,
        ThemeOptions {
            name: Some(theme_json.name.clone()),
            source_path,
        },
    )
}

/// `loadThemeFromPath`.
pub fn load_theme_from_path(theme_path: &Path, mode: Option<ColorMode>) -> Result<Theme, String> {
    let content = read_file(theme_path)?;
    let theme_json = parse_theme_json_content(&theme_path.display().to_string(), &content)?;
    create_theme(&theme_json, mode, Some(theme_path.to_path_buf()))
}

fn load_theme(name: &str, mode: Option<ColorMode>) -> Result<Arc<Theme>, String> {
    if let Some(theme) = registered_theme(name) {
        return Ok(theme);
    }
    let json = load_theme_json(name)?;
    Ok(Arc::new(create_theme(&json, mode, None)?))
}

/// `getThemeByName`.
pub fn get_theme_by_name(name: &str) -> Option<Arc<Theme>> {
    load_theme(name, None).ok()
}

/// `detectTerminalBackground` from `COLORFGBG`.
pub fn detect_terminal_background_with(colorfgbg: Option<&str>) -> &'static str {
    if let Some(value) = colorfgbg.filter(|v| !v.is_empty()) {
        let parts: Vec<&str> = value.split(';').collect();
        if parts.len() >= 2 {
            let digits: String = {
                let s = parts[1].trim_start();
                let (sign, rest) = match s.strip_prefix('-') {
                    Some(r) => ("-", r),
                    None => ("", s.strip_prefix('+').unwrap_or(s)),
                };
                let d: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if d.is_empty() {
                    String::new()
                } else {
                    format!("{sign}{d}")
                }
            };
            if let Ok(bg) = digits.parse::<i64>() {
                return if bg < 8 { "dark" } else { "light" };
            }
        }
    }
    "dark"
}

fn default_theme() -> &'static str {
    detect_terminal_background_with(std::env::var("COLORFGBG").ok().as_deref())
}

// ============================================================================
// Global theme
// ============================================================================

type ChangeCallback = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct State {
    theme: Option<Arc<Theme>>,
    current_name: Option<String>,
    registered: Vec<(String, Arc<Theme>)>,
    on_change: Option<ChangeCallback>,
    watcher: Option<Arc<AtomicBool>>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn registered_theme(name: &str) -> Option<Arc<Theme>> {
    state()
        .registered
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, t)| t.clone())
}

/// The current theme (`theme` in hoocode). Panics before [`init_theme`].
pub fn theme() -> Arc<Theme> {
    try_theme().expect("Theme not initialized. Call initTheme() first.")
}

/// The current theme, if one has been set.
pub fn try_theme() -> Option<Arc<Theme>> {
    state().theme.clone()
}

/// The name of the current theme (`"<in-memory>"` after
/// [`set_theme_instance`]).
pub fn current_theme_name() -> Option<String> {
    state().current_name.clone()
}

/// `setRegisteredThemes`: themes registered by extensions, by name.
pub fn set_registered_themes(themes: Vec<Theme>) {
    let mut s = state();
    s.registered.clear();
    for theme in themes {
        if let Some(name) = theme.name.clone() {
            let theme = Arc::new(theme);
            match s.registered.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = theme,
                None => s.registered.push((name, theme)),
            }
        }
    }
}

fn fallback_dark() -> Arc<Theme> {
    load_theme("dark", None).expect("the bundled dark theme loads")
}

/// `initTheme`: load the requested (or detected default) theme; an invalid
/// one falls back to `dark` silently.
pub fn init_theme(theme_name: Option<&str>, enable_watcher: bool) {
    let requested = theme_name.map_or_else(|| default_theme().to_string(), str::to_string);
    let name = resolve_theme_name(&requested);
    state().current_name = Some(name.clone());
    match load_theme(&name, None) {
        Ok(theme) => {
            state().theme = Some(theme);
            if enable_watcher {
                start_theme_watcher();
            }
        }
        Err(_) => {
            let dark = fallback_dark();
            let mut s = state();
            s.current_name = Some("dark".to_string());
            s.theme = Some(dark);
        }
    }
}

fn notify_change() {
    let callback = state().on_change.clone();
    if let Some(callback) = callback {
        callback();
    }
}

/// `setTheme`: switch themes; on failure fall back to `dark` and return the
/// error.
pub fn set_theme(requested: &str, enable_watcher: bool) -> Result<(), String> {
    let name = resolve_theme_name(requested);
    state().current_name = Some(name.clone());
    match load_theme(&name, None) {
        Ok(theme) => {
            state().theme = Some(theme);
            if enable_watcher {
                start_theme_watcher();
            }
            notify_change();
            Ok(())
        }
        Err(error) => {
            let dark = fallback_dark();
            let mut s = state();
            s.current_name = Some("dark".to_string());
            s.theme = Some(dark);
            Err(error)
        }
    }
}

/// `setThemeInstance`: use a theme object directly (not watchable).
pub fn set_theme_instance(theme: Theme) {
    {
        let mut s = state();
        s.theme = Some(Arc::new(theme));
        s.current_name = Some("<in-memory>".to_string());
    }
    stop_theme_watcher();
    notify_change();
}

/// `onThemeChange`: the callback run after the theme changes (it may run on
/// the watcher's thread).
pub fn on_theme_change(callback: impl Fn() + Send + Sync + 'static) {
    state().on_change = Some(Arc::new(callback));
}

/// File identity used to notice edits (the watcher polls; see
/// [`start_theme_watcher`]).
fn file_signature(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

/// How often the watcher looks at the file, and how long it waits for edits
/// to settle before reloading (hoocode's reload debounce).
const WATCH_POLL: Duration = Duration::from_millis(50);
const RELOAD_DEBOUNCE: Duration = Duration::from_millis(100);

/// `startThemeWatcher`: reload the current custom theme when its file
/// changes. Built-ins are not watched. Polls the file rather than using OS
/// notifications, so it needs no platform watcher.
fn start_theme_watcher() {
    stop_theme_watcher();
    let Some(watched_name) = current_theme_name() else {
        return;
    };
    if is_builtin_theme(&watched_name) {
        return;
    }
    let theme_file = custom_themes_dir().join(format!("{watched_name}.json"));
    if !theme_file.exists() {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    state().watcher = Some(stop.clone());
    std::thread::spawn(move || {
        let mut last = file_signature(&theme_file);
        let mut pending: Option<Instant> = None;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(WATCH_POLL);
            let signature = file_signature(&theme_file);
            if signature != last {
                last = signature;
                pending = Some(Instant::now());
            }
            if pending.is_some_and(|at| at.elapsed() >= RELOAD_DEBOUNCE) {
                pending = None;
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                reload_watched(&watched_name, &theme_file);
            }
        }
    });
}

fn reload_watched(watched_name: &str, theme_file: &Path) {
    // Ignore stale reloads after switching themes.
    if current_theme_name().as_deref() != Some(watched_name) {
        return;
    }
    // Keep the last good theme while the file is temporarily missing.
    if !theme_file.exists() {
        return;
    }
    // A file mid-edit may be invalid: keep the current theme.
    let Ok(reloaded) = load_theme_from_path(theme_file, None) else {
        return;
    };
    {
        let reloaded = Arc::new(reloaded);
        let mut s = state();
        match s.registered.iter_mut().find(|(n, _)| n == watched_name) {
            Some(slot) => slot.1 = reloaded.clone(),
            None => s
                .registered
                .push((watched_name.to_string(), reloaded.clone())),
        }
        s.theme = Some(reloaded);
    }
    notify_change();
}

/// `stopThemeWatcher`.
pub fn stop_theme_watcher() {
    if let Some(stop) = state().watcher.take() {
        stop.store(true, Ordering::Relaxed);
    }
}

// ============================================================================
// HTML export helpers
// ============================================================================

/// `hasLightBackdrop`: judged by the theme's export page, then
/// `userMessageBg`. A bad `export.pageBg` reference is an error, as in hoocode.
fn has_light_backdrop(
    theme_json: &ThemeJson,
    resolved: &[(String, RawColor)],
) -> Result<bool, String> {
    let vars = theme_json.vars.as_deref().unwrap_or(&[]);
    let page_bg = match theme_json.export.as_ref().and_then(|e| e.page_bg.as_ref()) {
        Some(v) => Some(resolve_var_refs(v, vars)?),
        None => None,
    };
    let user_bg = resolved
        .iter()
        .find(|(k, _)| k == "userMessageBg")
        .map(|(_, v)| v.clone());
    for candidate in [page_bg, user_bg].into_iter().flatten() {
        if candidate.is_default() {
            continue;
        }
        if let Some(luminance) = relative_luminance(&candidate) {
            return Ok(luminance > 0.5);
        }
    }
    Ok(false)
}

fn name_or_current(theme_name: Option<&str>) -> String {
    theme_name
        .map(str::to_string)
        .or_else(current_theme_name)
        .unwrap_or_else(|| default_theme().to_string())
}

/// `getResolvedThemeColors`: every token as a CSS hex string (256-color
/// indices converted, `""` replaced by a default text color).
pub fn get_resolved_theme_colors(
    theme_name: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let name = name_or_current(theme_name);
    let theme_json = load_theme_json(&name)?;
    let resolved = resolve_theme_colors(&theme_json.colors, theme_json.vars.as_deref())?;
    let default_text = if has_light_backdrop(&theme_json, &resolved)? {
        "#000000"
    } else {
        "#e5e5e7"
    };
    Ok(resolved
        .into_iter()
        .map(|(key, value)| {
            let css = match value {
                RawColor::Index(n) => ansi256_to_hex(n as i64),
                RawColor::Str(s) if s.is_empty() => default_text.to_string(),
                RawColor::Str(s) => s,
            };
            (key, css)
        })
        .collect())
}

/// Explicit export colors (`getThemeExportColors`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeExportColors {
    pub page_bg: Option<String>,
    pub card_bg: Option<String>,
    pub info_bg: Option<String>,
}

/// `getThemeExportColors`: the theme's `export` section resolved to hex;
/// each is `None` when unset (or `""`). Errors yield all `None`.
pub fn get_theme_export_colors(theme_name: Option<&str>) -> ThemeExportColors {
    let name = name_or_current(theme_name);
    let resolve_all = || -> Result<ThemeExportColors, String> {
        let theme_json = load_theme_json(&name)?;
        let Some(export) = &theme_json.export else {
            return Ok(ThemeExportColors::default());
        };
        let vars = theme_json.vars.as_deref().unwrap_or(&[]);
        let resolve = |value: &Option<ColorValue>| -> Result<Option<String>, String> {
            let Some(value) = value else { return Ok(None) };
            Ok(match resolve_var_refs(value, vars)? {
                RawColor::Index(n) => Some(ansi256_to_hex(n as i64)),
                RawColor::Str(s) if s.is_empty() => None,
                RawColor::Str(s) => Some(s),
            })
        };
        Ok(ThemeExportColors {
            page_bg: resolve(&export.page_bg)?,
            card_bg: resolve(&export.card_bg)?,
            info_bg: resolve(&export.info_bg)?,
        })
    };
    resolve_all().unwrap_or_default()
}
