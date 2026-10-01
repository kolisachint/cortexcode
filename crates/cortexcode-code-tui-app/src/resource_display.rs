//! The startup/reload resource listing (`resource-display.ts`): the counted
//! capability grid, context files, details one keypress away (expandable),
//! and resource diagnostics. Everything here is formatting; the data comes
//! in as a [`ResourceListing`].

use std::cell::RefCell;
use std::cmp::Ordering;
use std::path::Path;
use std::rc::Rc;

use cortexcode_code_agent_session::format::{
    format_tokens, render_compact_rows, CompactRowsOptions,
};
use cortexcode_code_paths::git::parse_git_url;
use cortexcode_code_resources::agent_registry::summarize_agent_description;
use cortexcode_code_resources::context_files::{ContextFile, ContextFileSize};
use cortexcode_code_resources::diagnostics::{DiagnosticType, ResourceDiagnostic};
use cortexcode_code_resources::source_info::{SourceInfo, SourceScope};
use cortexcode_code_tui_theme::theme;
use cortexcode_tui_components::{Spacer, Text};
use cortexcode_tui_render::ComponentHandle;
use cortexcode_tui_util::visible_width;

use crate::brand::{Category, SEGMENT_SEP};
use crate::expandable_text::ExpandableText;

/// Left rail for the summary (flush with the banner).
const RAIL: &str = "";

/// A loaded item: its path, where it came from, and an optional label.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ListedItem {
    pub name: String,
    pub path: String,
    pub source_info: Option<SourceInfo>,
    pub display_name: Option<String>,
}

/// A live MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerStatus {
    pub name: String,
    pub authorizing: bool,
    pub tool_count: usize,
    pub background: bool,
    pub deferred: bool,
}

/// A canvas that could be opened here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasEntry {
    pub id: String,
    pub scope: String,
    pub withheld: bool,
}

/// Everything the listing shows.
#[derive(Debug, Clone, Default)]
pub struct ResourceListing {
    pub cwd: String,
    pub skills: Vec<ListedItem>,
    pub skill_diagnostics: Vec<ResourceDiagnostic>,
    /// Prompt templates (`name` without the slash).
    pub templates: Vec<ListedItem>,
    pub prompt_diagnostics: Vec<ResourceDiagnostic>,
    pub context_files: Vec<ContextFile>,
    pub context_warnings: Vec<String>,
    /// Dispatchable agents (empty when the Task tool is off): name, description.
    pub agents: Vec<(String, String)>,
    pub mcp: Vec<McpServerStatus>,
    /// Loaded extensions (plugins have a `plugin:` display name).
    pub extensions: Vec<ListedItem>,
    pub extension_diagnostics: Vec<ResourceDiagnostic>,
    pub canvases: Vec<CanvasEntry>,
    /// Themes loaded from files (path + source).
    pub custom_themes: Vec<ListedItem>,
    pub theme_diagnostics: Vec<ResourceDiagnostic>,
    pub columns: Option<usize>,
    pub quiet_startup: bool,
    pub verbose: bool,
    /// The tool output dial is at full (verbose opens the details as well).
    pub expanded: bool,
}

/// `localeCompare`, approximately: case-insensitive, then exact.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}

fn home_dir() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok().filter(|h| !h.is_empty()))
}

/// `formatDisplayPath`: the home directory as `~`.
pub fn format_display_path(p: &str) -> String {
    match home_dir() {
        Some(home) if p.starts_with(&home) => format!("~{}", &p[home.len()..]),
        _ => p.to_string(),
    }
}

fn format_extension_display_path(p: &str) -> String {
    let result = format_display_path(p);
    result
        .strip_suffix("/index.ts")
        .or_else(|| result.strip_suffix("/index.js"))
        .map_or(result.clone(), str::to_string)
}

fn format_context_path(p: &str, cwd: &str) -> String {
    let path = Path::new(p);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(cwd).join(path)
    };
    match cortexcode_code_paths::cwd_relative_path(&absolute, Path::new(cwd)) {
        Some(relative) => relative.to_string_lossy().into_owned(),
        None => format_display_path(&absolute.to_string_lossy()),
    }
}

fn context_size_note(file: &ContextFile) -> String {
    let Some(size) = file.size else {
        return String::new();
    };
    let advice = if size == ContextFileSize::Truncated {
        "truncated"
    } else {
        "consider trimming"
    };
    let tokens = file.tokens.map(|t| format!("~{} tokens", format_tokens(t)));
    theme().fg(
        "warning",
        &format!(
            " {}{advice}",
            tokens
                .map(|t| format!("{t} {SEGMENT_SEP} "))
                .unwrap_or_default()
        ),
    )
}

fn is_package_source(info: Option<&SourceInfo>) -> bool {
    info.is_some_and(|i| i.source.starts_with("npm:") || i.source.starts_with("git:"))
}

/// `getShortPath`: a path relative to its package root when it has one.
fn get_short_path(full_path: &str, info: Option<&SourceInfo>) -> String {
    if let Some(info) = info {
        if let Some(base) = info
            .base_dir
            .as_deref()
            .filter(|_| is_package_source(Some(info)))
        {
            if let Ok(rel) = Path::new(full_path).strip_prefix(base) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                if !rel.is_empty() && rel != "." {
                    return rel;
                }
            }
        }
        if info.source.starts_with("npm:") {
            if let Some(i) = full_path.find("node_modules/") {
                let rest = &full_path[i + "node_modules/".len()..];
                let skip = if rest.starts_with('@') { 2 } else { 1 };
                let parts: Vec<&str> = rest.splitn(skip + 1, '/').collect();
                if parts.len() == skip + 1 {
                    return parts[skip].to_string();
                }
            }
        }
        if info.source.starts_with("git:") {
            if let Some(i) = full_path.find("git/") {
                let parts: Vec<&str> = full_path[i + 4..].splitn(3, '/').collect();
                if parts.len() == 3 {
                    return parts[2].to_string();
                }
            }
        }
    }
    format_display_path(full_path)
}

fn compact_path_label(path: &str, info: Option<&SourceInfo>) -> String {
    let short = get_short_path(path, info).replace('\\', "/");
    short
        .split('/')
        .rfind(|s| !s.is_empty() && *s != "~")
        .map_or(short.clone(), str::to_string)
}

fn compact_package_source_label(info: Option<&SourceInfo>) -> String {
    let source = info.map(|i| i.source.as_str()).unwrap_or("");
    if let Some(rest) = source.strip_prefix("npm:") {
        return if rest.is_empty() {
            source.to_string()
        } else {
            rest.to_string()
        };
    }
    match parse_git_url(source) {
        Some(git) if !git.path.is_empty() => git.path,
        _ => source.to_string(),
    }
}

fn compact_extension_label(path: &str, info: Option<&SourceInfo>) -> String {
    if !is_package_source(info) {
        return compact_path_label(path, info);
    }
    let label = compact_package_source_label(info);
    if label.is_empty() {
        return compact_path_label(path, info);
    }
    let short = get_short_path(path, info).replace('\\', "/");
    let package_path = short
        .strip_prefix("extensions/")
        .unwrap_or(&short)
        .to_string();
    let (dir, file) = match package_path.rfind('/') {
        Some(i) => (&package_path[..i], &package_path[i + 1..]),
        None => ("", package_path.as_str()),
    };
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    if stem == "index" {
        return if dir.is_empty() || dir == "." {
            label
        } else {
            format!("{label}:{dir}")
        };
    }
    format!("{label}:{package_path}")
}

fn display_path_segments(path: &str) -> Vec<String> {
    format_display_path(path)
        .replace('\\', "/")
        .split('/')
        .filter(|s| !s.is_empty() && *s != "~")
        .map(str::to_string)
        .collect()
}

fn compact_extension_labels(extensions: &[ListedItem]) -> Vec<String> {
    let non_package: Vec<(String, Vec<String>)> = extensions
        .iter()
        .filter(|e| !is_package_source(e.source_info.as_ref()))
        .map(|e| {
            let mut segments = display_path_segments(&e.path);
            if segments.len() > 1
                && matches!(
                    segments.last().map(String::as_str),
                    Some("index.ts" | "index.js")
                )
            {
                segments.pop();
            }
            (e.path.clone(), segments)
        })
        .collect();
    extensions
        .iter()
        .map(|e| {
            if let Some(name) = &e.display_name {
                return name.clone();
            }
            if is_package_source(e.source_info.as_ref()) {
                return compact_extension_label(&e.path, e.source_info.as_ref());
            }
            let Some(index) = non_package.iter().position(|(p, _)| *p == e.path) else {
                return compact_path_label(&e.path, e.source_info.as_ref());
            };
            let segments = &non_package[index].1;
            if segments.is_empty() {
                return compact_path_label(&e.path, None);
            }
            for count in 1..=segments.len() {
                let candidate = segments[segments.len() - count..].join("/");
                let unique = non_package.iter().enumerate().all(|(i, (_, other))| {
                    i == index || other[other.len().saturating_sub(count)..].join("/") != candidate
                });
                if unique {
                    return candidate;
                }
            }
            segments.join("/")
        })
        .collect()
}

/// (label, scope label).
fn display_source_info(info: Option<&SourceInfo>) -> (String, Option<&'static str>) {
    let source = info.map(|i| i.source.as_str()).unwrap_or("local");
    let scope = info.map(|i| i.scope).unwrap_or(SourceScope::Project);
    match source {
        "local" => match scope {
            SourceScope::User => ("user".into(), None),
            SourceScope::Project => ("project".into(), None),
            SourceScope::Temporary => ("path".into(), Some("temp")),
        },
        "cli" => (
            "path".into(),
            (scope == SourceScope::Temporary).then_some("temp"),
        ),
        other => (
            other.to_string(),
            Some(match scope {
                SourceScope::User => "user",
                SourceScope::Project => "project",
                SourceScope::Temporary => "temp",
            }),
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ScopeGroupKind {
    User,
    Project,
    Path,
}

impl ScopeGroupKind {
    fn as_str(self) -> &'static str {
        match self {
            ScopeGroupKind::User => "user",
            ScopeGroupKind::Project => "project",
            ScopeGroupKind::Path => "path",
        }
    }
}

fn scope_group(info: Option<&SourceInfo>) -> ScopeGroupKind {
    let source = info.map(|i| i.source.as_str()).unwrap_or("local");
    let scope = info.map(|i| i.scope).unwrap_or(SourceScope::Project);
    if source == "cli" || scope == SourceScope::Temporary {
        return ScopeGroupKind::Path;
    }
    match scope {
        SourceScope::User => ScopeGroupKind::User,
        SourceScope::Project => ScopeGroupKind::Project,
        SourceScope::Temporary => ScopeGroupKind::Path,
    }
}

struct ScopeGroup<'a> {
    scope: ScopeGroupKind,
    paths: Vec<&'a ListedItem>,
    packages: Vec<(String, Vec<&'a ListedItem>)>,
}

fn build_scope_groups(items: &[ListedItem]) -> Vec<ScopeGroup<'_>> {
    let mut groups: Vec<ScopeGroup<'_>> = [
        ScopeGroupKind::Project,
        ScopeGroupKind::User,
        ScopeGroupKind::Path,
    ]
    .into_iter()
    .map(|scope| ScopeGroup {
        scope,
        paths: Vec::new(),
        packages: Vec::new(),
    })
    .collect();
    for item in items {
        let kind = scope_group(item.source_info.as_ref());
        let group = groups
            .iter_mut()
            .find(|g| g.scope == kind)
            .expect("three groups");
        if is_package_source(item.source_info.as_ref()) {
            let source = item
                .source_info
                .as_ref()
                .map(|i| i.source.clone())
                .unwrap_or_default();
            match group.packages.iter_mut().find(|(s, _)| *s == source) {
                Some((_, list)) => list.push(item),
                None => group.packages.push((source, vec![item])),
            }
        } else {
            group.paths.push(item);
        }
    }
    groups.retain(|g| !g.paths.is_empty() || !g.packages.is_empty());
    groups
}

fn format_scope_groups(
    groups: &[ScopeGroup<'_>],
    format_path: &dyn Fn(&ListedItem) -> String,
    format_package_path: &dyn Fn(&ListedItem) -> String,
) -> String {
    let t = theme();
    let mut lines = Vec::new();
    for group in groups {
        lines.push(format!("  {}", t.fg("accent", group.scope.as_str())));
        let mut paths = group.paths.clone();
        paths.sort_by(|a, b| locale_cmp(&a.path, &b.path));
        for item in paths {
            lines.push(t.fg("dim", &format!("    {}", format_path(item))));
        }
        let mut packages: Vec<&(String, Vec<&ListedItem>)> = group.packages.iter().collect();
        packages.sort_by(|a, b| locale_cmp(&a.0, &b.0));
        for (source, items) in packages {
            lines.push(format!("    {}", t.fg("mdLink", source)));
            let mut items = items.clone();
            items.sort_by(|a, b| locale_cmp(&a.path, &b.path));
            for item in items {
                lines.push(t.fg("dim", &format!("      {}", format_package_path(item))));
            }
        }
    }
    lines.join("\n")
}

fn find_source_info<'a>(p: &str, infos: &'a [(String, SourceInfo)]) -> Option<&'a SourceInfo> {
    let get = |key: &str| infos.iter().find(|(k, _)| k == key).map(|(_, i)| i);
    if let Some(exact) = get(p) {
        return Some(exact);
    }
    let mut current = p;
    while let Some(i) = current.rfind('/') {
        current = &current[..i];
        if let Some(parent) = get(current) {
            return Some(parent);
        }
    }
    None
}

fn format_path_with_source(p: &str, info: Option<&SourceInfo>) -> String {
    match info {
        Some(info) => {
            let short = get_short_path(p, Some(info));
            let (label, scope) = display_source_info(Some(info));
            let label = match scope {
                Some(scope) => format!("{label} ({scope})"),
                None => label,
            };
            format!("{label} {short}")
        }
        None => format_display_path(p),
    }
}

/// `formatDiagnostics`: collisions grouped by name, then the rest.
fn format_diagnostics(
    diagnostics: &[ResourceDiagnostic],
    infos: &[(String, SourceInfo)],
) -> String {
    let t = theme();
    let mut lines = Vec::new();
    let mut collisions: Vec<(String, Vec<&ResourceDiagnostic>)> = Vec::new();
    let mut others = Vec::new();
    for d in diagnostics {
        match (&d.kind, &d.collision) {
            (DiagnosticType::Collision, Some(c)) => {
                match collisions.iter_mut().find(|(n, _)| *n == c.name) {
                    Some((_, list)) => list.push(d),
                    None => collisions.push((c.name.clone(), vec![d])),
                }
            }
            _ => others.push(d),
        }
    }
    for (name, list) in &collisions {
        let Some(first) = list[0].collision.as_ref() else {
            continue;
        };
        lines.push(t.fg("warning", &format!("  \"{name}\" collision:")));
        lines.push(t.fg(
            "dim",
            &format!(
                "    {} {}",
                t.fg("success", "✓"),
                format_path_with_source(
                    &first.winner_path,
                    find_source_info(&first.winner_path, infos)
                )
            ),
        ));
        for d in list {
            if let Some(c) = &d.collision {
                lines.push(t.fg(
                    "dim",
                    &format!(
                        "    {} {} (skipped)",
                        t.fg("warning", "✗"),
                        format_path_with_source(
                            &c.loser_path,
                            find_source_info(&c.loser_path, infos)
                        )
                    ),
                ));
            }
        }
    }
    for d in others {
        let color = if d.kind == DiagnosticType::Error {
            "error"
        } else {
            "warning"
        };
        match &d.path {
            Some(path) => {
                let formatted = format_path_with_source(path, find_source_info(path, infos));
                lines.push(t.fg(color, &format!("  {formatted}")));
                lines.push(t.fg(color, &format!("    {}", d.message)));
            }
            None => lines.push(t.fg(color, &format!("  {}", d.message))),
        }
    }
    lines.join("\n")
}

fn handle<C: cortexcode_tui_render::Component + 'static>(c: C) -> ComponentHandle {
    Rc::new(RefCell::new(c))
}

/// `willShowResourceListing`.
pub fn will_show_resource_listing(listing: &ResourceListing, force: bool) -> bool {
    force || listing.verbose || !listing.quiet_startup
}

/// `showLoadedResources`: the components to append to the transcript.
pub fn show_loaded_resources(
    listing: &ResourceListing,
    force: bool,
    show_diagnostics_when_quiet: bool,
) -> Vec<ComponentHandle> {
    let show_listing = will_show_resource_listing(listing, force);
    let show_diagnostics = show_listing || show_diagnostics_when_quiet;
    let mut out: Vec<ComponentHandle> = Vec::new();
    if !show_listing && !show_diagnostics {
        return out;
    }
    let t = theme();
    let section_header = |name: &str| t.fg("mdHeading", &format!("[{name}]"));
    let format_compact_list = |items: Vec<String>, sort: bool| {
        let mut labels: Vec<String> = items
            .into_iter()
            .map(|i| i.trim().to_string())
            .filter(|i| !i.is_empty())
            .collect();
        if sort {
            labels.sort_by(|a, b| locale_cmp(a, b));
        }
        t.fg("dim", &format!("  {}", labels.join(", ")))
    };

    let mut source_infos: Vec<(String, SourceInfo)> = Vec::new();
    for item in listing
        .extensions
        .iter()
        .chain(&listing.skills)
        .chain(&listing.templates)
        .chain(&listing.custom_themes)
    {
        if let Some(info) = &item.source_info {
            source_infos.retain(|(k, _)| *k != item.path);
            source_infos.push((item.path.clone(), info.clone()));
        }
    }

    if show_listing {
        out.push(handle(Spacer::new(1)));
        let is_plugin = |e: &ListedItem| {
            e.display_name
                .as_deref()
                .is_some_and(|n| n.starts_with("plugin:"))
        };
        let plugin_count = listing.extensions.iter().filter(|e| is_plugin(e)).count();
        let code_extension_count = listing.extensions.len() - plugin_count;
        let plural = |n: usize, s: &str, many: Option<&str>| {
            if n == 1 {
                s.to_string()
            } else {
                many.map_or_else(|| format!("{s}s"), str::to_string)
            }
        };
        let mut cells: Vec<(Category, usize, String)> = Vec::new();
        let skills = listing.skills.len();
        if skills > 0 {
            cells.push((Category::Skills, skills, plural(skills, "skill", None)));
        }
        let templates = listing.templates.len();
        if templates > 0 {
            cells.push((
                Category::Commands,
                templates,
                plural(templates, "command", None),
            ));
        }
        let agents = listing.agents.len();
        if agents > 0 {
            cells.push((Category::Agents, agents, plural(agents, "agent", None)));
        }
        if !listing.mcp.is_empty() {
            cells.push((
                Category::Mcp,
                listing.mcp.len(),
                plural(listing.mcp.len(), "mcp server", None),
            ));
        }
        if plugin_count > 0 {
            cells.push((
                Category::Plugins,
                plugin_count,
                plural(plugin_count, "plugin", None),
            ));
        }
        if code_extension_count > 0 {
            cells.push((
                Category::Extensions,
                code_extension_count,
                plural(code_extension_count, "extension", None),
            ));
        }
        if !listing.canvases.is_empty() {
            let n = listing.canvases.len();
            cells.push((Category::Canvases, n, plural(n, "canvas", Some("canvases"))));
        }
        if !listing.custom_themes.is_empty() {
            let n = listing.custom_themes.len();
            cells.push((Category::Themes, n, plural(n, "theme", None)));
        }

        if cells.is_empty() {
            out.push(handle(Text::new(
                t.fg("dim", &format!("{RAIL}no project resources loaded")),
                0,
                0,
            )));
        } else {
            let cell_plain = |(key, count, label): &(Category, usize, String)| {
                format!("{} {count} {label}", key.glyph())
            };
            let cell_width = cells
                .iter()
                .map(|c| visible_width(&cell_plain(c)))
                .max()
                .unwrap_or(0)
                + 3;
            let styled_cell = |c: &(Category, usize, String)| {
                let pad = " ".repeat(cell_width.saturating_sub(visible_width(&cell_plain(c))));
                format!(
                    "{} {} {}{pad}",
                    t.fg("accent", c.0.glyph()),
                    t.bold(&c.1.to_string()),
                    t.fg("muted", &c.2)
                )
            };
            let rows: Vec<String> = cells
                .chunks(4)
                .map(|row| {
                    format!("{RAIL}{}", row.iter().map(styled_cell).collect::<String>())
                        .trim_end()
                        .to_string()
                })
                .collect();
            out.push(handle(Text::new(rows.join("\n"), 0, 0)));
        }

        if !listing.context_files.is_empty() {
            let names = listing
                .context_files
                .iter()
                .map(|f| format_context_path(&f.path, &listing.cwd) + &context_size_note(f))
                .collect::<Vec<_>>()
                .join(", ");
            let total_tokens: u64 = listing
                .context_files
                .iter()
                .map(|f| f.tokens.unwrap_or(0))
                .sum();
            let total = if total_tokens > 0 {
                t.fg(
                    "muted",
                    &format!(
                        " {SEGMENT_SEP} ~{} tokens/turn",
                        format_tokens(total_tokens)
                    ),
                )
            } else {
                String::new()
            };
            out.push(handle(Text::new(
                format!(
                    "{RAIL}{} {} {names}{total}",
                    t.fg("accent", Category::Context.glyph()),
                    t.fg("muted", "context")
                ),
                0,
                0,
            )));
        }

        let mut detail_sections: Vec<String> = Vec::new();
        if !listing.skills.is_empty() {
            let groups = build_scope_groups(&listing.skills);
            let list = format_scope_groups(&groups, &|i| format_display_path(&i.path), &|i| {
                get_short_path(&i.path, i.source_info.as_ref())
            });
            detail_sections.push(format!(
                "{}\n{}\n{list}",
                section_header("Skills"),
                format_compact_list(
                    listing.skills.iter().map(|s| s.name.clone()).collect(),
                    true
                )
            ));
        }
        if !listing.templates.is_empty() {
            let groups = build_scope_groups(&listing.templates);
            let format_template = |i: &ListedItem| format!("/{}", i.name);
            let list = format_scope_groups(&groups, &format_template, &format_template);
            detail_sections.push(format!(
                "{}\n{}\n{list}",
                section_header("Commands"),
                format_compact_list(
                    listing
                        .templates
                        .iter()
                        .map(|t| format!("/{}", t.name))
                        .collect(),
                    true
                )
            ));
        }
        if !listing.agents.is_empty() {
            let summaries: Vec<(String, String)> = listing
                .agents
                .iter()
                .map(|(name, description)| (name.clone(), summarize_agent_description(description)))
                .collect();
            let rows: Vec<(&str, &str)> = summaries
                .iter()
                .map(|(n, d)| (n.as_str(), d.as_str()))
                .collect();
            let muted = |s: &str| theme().fg("muted", s);
            let dim = |s: &str| theme().fg("dim", s);
            let list = render_compact_rows(
                &rows,
                &CompactRowsOptions {
                    columns: listing.columns,
                    name_style: Some(&muted),
                    detail_style: Some(&dim),
                    ..Default::default()
                },
            );
            detail_sections.push(format!("{}\n{list}", section_header("Agents")));
        }
        if !listing.mcp.is_empty() {
            let list = listing
                .mcp
                .iter()
                .map(|server| {
                    let facts: Vec<String> = if server.authorizing {
                        vec!["awaiting authorization".into()]
                    } else {
                        let mut f = vec![
                            format!(
                                "{} {}",
                                server.tool_count,
                                plural(server.tool_count, "tool", None)
                            ),
                            if server.background {
                                "background"
                            } else {
                                "foreground"
                            }
                            .to_string(),
                        ];
                        if server.deferred {
                            f.push("schemas deferred".into());
                        }
                        f
                    };
                    t.fg(
                        "dim",
                        &format!(
                            "  {} {}",
                            server.name,
                            facts.join(&format!(" {SEGMENT_SEP} "))
                        ),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            detail_sections.push(format!("{}\n{list}", section_header("MCP")));
        }
        let plugins: Vec<ListedItem> = listing
            .extensions
            .iter()
            .filter(|e| is_plugin(e))
            .cloned()
            .collect();
        let extension_path = |i: &ListedItem| {
            i.display_name
                .clone()
                .unwrap_or_else(|| format_extension_display_path(&i.path))
        };
        let extension_package_path = |i: &ListedItem| {
            i.display_name.clone().unwrap_or_else(|| {
                format_extension_display_path(&get_short_path(&i.path, i.source_info.as_ref()))
            })
        };
        if !plugins.is_empty() {
            let list = format_scope_groups(
                &build_scope_groups(&plugins),
                &extension_path,
                &extension_package_path,
            );
            detail_sections.push(format!("{}\n{list}", section_header("Plugins")));
        }
        if !listing.extensions.is_empty() {
            let list = format_scope_groups(
                &build_scope_groups(&listing.extensions),
                &extension_path,
                &extension_package_path,
            );
            detail_sections.push(format!(
                "{}\n{}\n{list}",
                section_header("Extensions"),
                format_compact_list(compact_extension_labels(&listing.extensions), true)
            ));
        }
        if !listing.custom_themes.is_empty() {
            let list = format_scope_groups(
                &build_scope_groups(&listing.custom_themes),
                &|i| format_display_path(&i.path),
                &|i| get_short_path(&i.path, i.source_info.as_ref()),
            );
            detail_sections.push(format!("{}\n{list}", section_header("Themes")));
        }
        if !listing.canvases.is_empty() {
            let details: Vec<(String, String)> = listing
                .canvases
                .iter()
                .map(|c| {
                    let detail = if c.withheld {
                        "withheld: untrusted workspace — /plugin trust".to_string()
                    } else {
                        format!("{} · /canvas open {}", c.scope, c.id)
                    };
                    (c.id.clone(), detail)
                })
                .collect();
            let rows: Vec<(&str, &str)> = details
                .iter()
                .map(|(n, d)| (n.as_str(), d.as_str()))
                .collect();
            let muted = |s: &str| theme().fg("muted", s);
            let dim = |s: &str| theme().fg("dim", s);
            let list = render_compact_rows(
                &rows,
                &CompactRowsOptions {
                    columns: listing.columns,
                    name_style: Some(&muted),
                    detail_style: Some(&dim),
                    ..Default::default()
                },
            );
            detail_sections.push(format!("{}\n{list}", section_header("Canvases")));
        }
        if !detail_sections.is_empty() {
            let details = detail_sections.join("\n\n");
            out.push(handle(ExpandableText::new(
                String::new,
                move || details.clone(),
                // `getStartupExpansionState`: verbose, or the dial at full.
                listing.verbose || listing.expanded,
                0,
                0,
            )));
        }
        for warning in &listing.context_warnings {
            out.push(handle(Text::new(
                t.fg("warning", &format!("{RAIL}{warning}")),
                0,
                0,
            )));
        }
    }

    if show_diagnostics {
        for (title, diagnostics) in [
            ("[Skill conflicts]", &listing.skill_diagnostics),
            ("[Prompt conflicts]", &listing.prompt_diagnostics),
            ("[Extension issues]", &listing.extension_diagnostics),
            ("[Theme conflicts]", &listing.theme_diagnostics),
        ] {
            if diagnostics.is_empty() {
                continue;
            }
            let lines = format_diagnostics(diagnostics, &source_infos);
            out.push(handle(Text::new(
                format!("{}\n{lines}", t.fg("warning", title)),
                0,
                0,
            )));
            out.push(handle(Spacer::new(1)));
        }
    }
    out
}
