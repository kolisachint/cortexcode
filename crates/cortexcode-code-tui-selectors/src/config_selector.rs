//! The `config` subcommand's resource list, hoocode
//! `components/config-selector.ts`: every resolved extension, skill, prompt
//! and theme, grouped by where it came from, each with a checkbox that
//! writes a `+pattern` / `-pattern` entry to the settings file it belongs to.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

use cortexcode_code_paths::CONFIG_DIR_NAME;
use cortexcode_code_resources::node_path;
use cortexcode_code_resources::package_resolve::{PathMetadata, ResolvedPaths, ResolvedResource};
use cortexcode_code_resources::{SourceOrigin, SourceScope};
use cortexcode_code_settings::{PackageFilter, PackageSource, SettingsManager};
use cortexcode_code_tui_keybindings::raw_key_hint;
use cortexcode_code_tui_theme::{
    paint_selected_row, select_gutter, style_input, theme, SELECT_CURSOR,
};
use cortexcode_tui_components::{Input, Spacer};
use cortexcode_tui_keys::{get_keybindings, matches_key};
use cortexcode_tui_render::{Component, ComponentHandle, Container};
use cortexcode_tui_util::{truncate_to_width, visible_width};
use serde_json::Value;

/// The four resource kinds the list shows, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceType {
    Extensions,
    Skills,
    Prompts,
    Themes,
}

impl ResourceType {
    /// The settings key and the search word (`extensions`, ...).
    pub fn key(self) -> &'static str {
        match self {
            Self::Extensions => "extensions",
            Self::Skills => "skills",
            Self::Prompts => "prompts",
            Self::Themes => "themes",
        }
    }

    /// `RESOURCE_TYPE_LABELS`.
    fn label(self) -> &'static str {
        match self {
            Self::Extensions => "Extensions",
            Self::Skills => "Skills",
            Self::Prompts => "Prompts",
            Self::Themes => "Themes",
        }
    }
}

/// `ResourceItem`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceItem {
    pub path: String,
    pub enabled: bool,
    pub metadata: PathMetadata,
    pub resource_type: ResourceType,
    pub display_name: String,
}

/// `ResourceSubgroup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceSubgroup {
    pub resource_type: ResourceType,
    pub label: String,
    pub items: Vec<ResourceItem>,
}

/// `ResourceGroup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceGroup {
    pub key: String,
    pub label: String,
    pub scope: SourceScope,
    pub origin: SourceOrigin,
    pub source: String,
    pub subgroups: Vec<ResourceSubgroup>,
}

/// An approximation of `String.prototype.localeCompare` (ICU root
/// collation): punctuation, digits, letters; case-insensitive first, then
/// lowercase before uppercase.
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    fn key(c: char) -> (u8, char) {
        let class = if c.is_alphabetic() {
            2
        } else if c.is_numeric() {
            1
        } else {
            0
        };
        (class, c.to_lowercase().next().unwrap_or(c))
    }
    a.chars().map(key).cmp(b.chars().map(key)).then_with(|| {
        let case = |s: &str| s.chars().map(char::is_uppercase).collect::<Vec<_>>();
        case(a).cmp(&case(b))
    })
}

/// `Array.prototype.sort` for short arrays: V8 sorts them with a binary
/// insertion sort, which also gives a defined order for a comparator that is
/// not a total order (the group comparator below is not).
fn js_sort<T>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> Ordering) {
    for i in 1..items.len() {
        let (mut left, mut right) = (0, i);
        while left < right {
            let mid = left + (right - left) / 2;
            if compare(&items[i], &items[mid]) == Ordering::Less {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        items[left..=i].rotate_right(1);
    }
}

fn home_dir() -> String {
    cortexcode_code_resources::package_resolve::home_dir()
}

/// `formatBaseDir`: `~` for home, `/` separators, a trailing `/`.
fn format_base_dir(base_dir: &str) -> String {
    let home = home_dir();
    let display = if base_dir == home {
        "~".to_string()
    } else if let Some(rest) = base_dir.strip_prefix(&home) {
        format!("~{}", rest.replace('\\', "/"))
    } else {
        base_dir.replace('\\', "/")
    };
    if display.ends_with('/') {
        display
    } else {
        format!("{display}/")
    }
}

fn scope_str(scope: SourceScope) -> &'static str {
    match scope {
        SourceScope::User => "user",
        SourceScope::Project => "project",
        SourceScope::Temporary => "temporary",
    }
}

fn origin_str(origin: SourceOrigin) -> &'static str {
    match origin {
        SourceOrigin::Package => "package",
        SourceOrigin::TopLevel => "top-level",
        SourceOrigin::ClaudeCode => "claude-code",
    }
}

/// `getGroupLabel`.
fn group_label(metadata: &PathMetadata) -> String {
    let user = metadata.scope == SourceScope::User;
    if metadata.origin == SourceOrigin::Package {
        return format!("{} ({})", metadata.source, scope_str(metadata.scope));
    }
    if metadata.source == "auto" {
        if let Some(base) = &metadata.base_dir {
            let base = format_base_dir(base);
            return if user {
                format!("User ({base})")
            } else {
                format!("Project ({base})")
            };
        }
        return if user {
            format!("User (~/{CONFIG_DIR_NAME}/agent/)")
        } else {
            format!("Project ({CONFIG_DIR_NAME}/)")
        };
    }
    if user {
        "User settings"
    } else {
        "Project settings"
    }
    .to_string()
}

/// `buildGroups`: resources grouped by origin, scope, source and base dir;
/// packages first, user before project; items by name.
pub fn build_groups(resolved: &ResolvedPaths) -> Vec<ResourceGroup> {
    let mut groups: Vec<ResourceGroup> = Vec::new();
    let mut add = |resources: &[ResolvedResource], resource_type: ResourceType| {
        for res in resources {
            let m = &res.metadata;
            let key = format!(
                "{}:{}:{}:{}",
                origin_str(m.origin),
                scope_str(m.scope),
                m.source,
                m.base_dir.as_deref().unwrap_or("")
            );
            let index = match groups.iter().position(|g| g.key == key) {
                Some(index) => index,
                None => {
                    groups.push(ResourceGroup {
                        key: key.clone(),
                        label: group_label(m),
                        scope: m.scope,
                        origin: m.origin,
                        source: m.source.clone(),
                        subgroups: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            let group = &mut groups[index];
            let subgroup = match group
                .subgroups
                .iter()
                .position(|s| s.resource_type == resource_type)
            {
                Some(i) => &mut group.subgroups[i],
                None => {
                    group.subgroups.push(ResourceSubgroup {
                        resource_type,
                        label: resource_type.label().to_string(),
                        items: Vec::new(),
                    });
                    group.subgroups.last_mut().unwrap()
                }
            };
            let file_name = node_path::basename(&res.path);
            let parent = node_path::basename(&node_path::dirname(&res.path));
            let display_name =
                if resource_type == ResourceType::Extensions && parent != "extensions" {
                    format!("{parent}/{file_name}")
                } else if resource_type == ResourceType::Skills && file_name == "SKILL.md" {
                    parent
                } else {
                    file_name
                };
            subgroup.items.push(ResourceItem {
                path: res.path.clone(),
                enabled: res.enabled,
                metadata: m.clone(),
                resource_type,
                display_name,
            });
        }
    };
    add(&resolved.extensions, ResourceType::Extensions);
    add(&resolved.skills, ResourceType::Skills);
    add(&resolved.prompts, ResourceType::Prompts);
    add(&resolved.themes, ResourceType::Themes);

    js_sort(&mut groups, |a, b| {
        if a.origin != b.origin {
            return if a.origin == SourceOrigin::Package {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        if a.scope != b.scope {
            return if a.scope == SourceScope::User {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        locale_compare(&a.source, &b.source)
    });
    for group in &mut groups {
        js_sort(&mut group.subgroups, |a, b| {
            a.resource_type.cmp(&b.resource_type)
        });
        for subgroup in &mut group.subgroups {
            js_sort(&mut subgroup.items, |a, b| {
                locale_compare(&a.display_name, &b.display_name)
            });
        }
    }
    groups
}

/// `ConfigSelectorHeader`: the title with the keys on the right, and the
/// search hint.
struct Header;

impl Component for Header {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let width = width as usize;
        let title = t.bold("Resource Configuration");
        let hint =
            raw_key_hint("space", "toggle") + &t.fg("muted", " · ") + &raw_key_hint("esc", "close");
        let spacing = width
            .saturating_sub(visible_width(&title) + visible_width(&hint))
            .max(1);
        vec![
            truncate_to_width(
                &format!("{title}{}{hint}", " ".repeat(spacing)),
                width,
                "",
                false,
            ),
            t.fg("muted", "Type to filter resources"),
        ]
    }
}

/// One row of the list: a group header, a type header, or a resource
/// (indices into `groups`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    Group(usize),
    Subgroup(usize, usize),
    Item(usize, usize, usize),
}

/// Called after a resource is toggled, with its new state.
pub type OnToggle = Box<dyn FnMut(&ResourceItem, bool)>;

/// `ResourceList`: the searchable, toggleable list the keyboard goes to.
pub struct ResourceList {
    groups: Vec<ResourceGroup>,
    flat: Vec<Entry>,
    filtered: Vec<Entry>,
    selected: usize,
    search: Input,
    max_visible: usize,
    settings: Rc<RefCell<SettingsManager>>,
    cwd: String,
    agent_dir: String,
    pub on_cancel: Option<Box<dyn FnMut()>>,
    pub on_exit: Option<Box<dyn FnMut()>>,
    pub on_toggle: Option<OnToggle>,
}

impl ResourceList {
    fn new(
        groups: Vec<ResourceGroup>,
        settings: Rc<RefCell<SettingsManager>>,
        cwd: &str,
        agent_dir: &str,
    ) -> Self {
        let mut search = Input::new();
        style_input(&mut search);
        let mut flat = Vec::new();
        for (g, group) in groups.iter().enumerate() {
            flat.push(Entry::Group(g));
            for (s, subgroup) in group.subgroups.iter().enumerate() {
                flat.push(Entry::Subgroup(g, s));
                flat.extend((0..subgroup.items.len()).map(|i| Entry::Item(g, s, i)));
            }
        }
        let mut list = Self {
            groups,
            filtered: flat.clone(),
            flat,
            selected: 0,
            search,
            max_visible: 15,
            settings,
            cwd: cwd.to_string(),
            agent_dir: agent_dir.to_string(),
            on_cancel: None,
            on_exit: None,
            on_toggle: None,
        };
        list.select_first_item();
        list
    }

    /// The groups, with each item's current state.
    pub fn groups(&self) -> &[ResourceGroup] {
        &self.groups
    }

    fn item(&self, entry: Entry) -> Option<&ResourceItem> {
        match entry {
            Entry::Item(g, s, i) => Some(&self.groups[g].subgroups[s].items[i]),
            _ => None,
        }
    }

    fn is_item(entry: &Entry) -> bool {
        matches!(entry, Entry::Item(..))
    }

    /// `findNextItem`: the next resource row in `direction`, or stay.
    fn find_next_item(&self, from: usize, forward: bool) -> usize {
        let mut idx = from as isize;
        loop {
            idx += if forward { 1 } else { -1 };
            if idx < 0 || idx as usize >= self.filtered.len() {
                return from;
            }
            if Self::is_item(&self.filtered[idx as usize]) {
                return idx as usize;
            }
        }
    }

    fn select_first_item(&mut self) {
        self.selected = self.filtered.iter().position(Self::is_item).unwrap_or(0);
    }

    /// `filterItems`: resources whose name, type or path contains the query,
    /// with the headers above them.
    fn filter_items(&mut self, query: &str) {
        if query.trim().is_empty() {
            self.filtered = self.flat.clone();
            self.select_first_item();
            return;
        }
        let query = query.to_lowercase();
        let matches = |item: &ResourceItem| {
            item.display_name.to_lowercase().contains(&query)
                || item.resource_type.key().contains(&query)
                || item.path.to_lowercase().contains(&query)
        };
        let hit = |g: usize, s: Option<usize>| {
            self.groups[g]
                .subgroups
                .iter()
                .enumerate()
                .filter(|(si, _)| s.is_none_or(|s| s == *si))
                .any(|(_, sub)| sub.items.iter().any(matches))
        };
        self.filtered = self
            .flat
            .iter()
            .copied()
            .filter(|entry| match *entry {
                Entry::Group(g) => hit(g, None),
                Entry::Subgroup(g, s) => hit(g, Some(s)),
                Entry::Item(..) => self.item(*entry).is_some_and(matches),
            })
            .collect();
        self.select_first_item();
    }

    /// `toggleResource`.
    fn toggle_resource(&self, item: &ResourceItem, enabled: bool) {
        if item.metadata.origin == SourceOrigin::TopLevel {
            self.toggle_top_level_resource(item, enabled);
        } else {
            self.toggle_package_resource(item, enabled);
        }
    }

    fn top_level_base_dir(&self, scope: SourceScope) -> String {
        if scope == SourceScope::Project {
            node_path::join(&[&self.cwd, CONFIG_DIR_NAME])
        } else {
            self.agent_dir.clone()
        }
    }

    /// Drop the existing `!`/`+`/`-` entries for `pattern`, then add the new one.
    fn with_toggle(current: &[String], pattern: &str, enabled: bool) -> Vec<String> {
        let mut updated: Vec<String> = current
            .iter()
            .filter(|p| {
                let stripped = p.strip_prefix(['!', '+', '-']).unwrap_or(p);
                stripped != pattern
            })
            .cloned()
            .collect();
        updated.push(format!("{}{pattern}", if enabled { '+' } else { '-' }));
        updated
    }

    /// `toggleTopLevelResource`: a pattern in the scope's resource array.
    fn toggle_top_level_resource(&self, item: &ResourceItem, enabled: bool) {
        let project = item.metadata.scope == SourceScope::Project;
        let mut settings = self.settings.borrow_mut();
        let source = if project {
            settings.project_settings()
        } else {
            settings.global_settings()
        };
        let key = item.resource_type.key();
        let current: Vec<String> = source
            .get(key)
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let pattern =
            node_path::relative(&self.top_level_base_dir(item.metadata.scope), &item.path);
        let updated = Self::with_toggle(&current, &pattern, enabled);
        match (project, item.resource_type) {
            (true, ResourceType::Extensions) => settings.set_project_extension_paths(&updated),
            (true, ResourceType::Skills) => settings.set_project_skill_paths(&updated),
            (true, ResourceType::Prompts) => settings.set_project_prompt_template_paths(&updated),
            (true, ResourceType::Themes) => settings.set_project_theme_paths(&updated),
            (false, ResourceType::Extensions) => settings.set_extension_paths(&updated),
            (false, ResourceType::Skills) => settings.set_skill_paths(&updated),
            (false, ResourceType::Prompts) => settings.set_prompt_template_paths(&updated),
            (false, ResourceType::Themes) => settings.set_theme_paths(&updated),
        }
    }

    /// `togglePackageResource`: a pattern in the package entry's filter,
    /// relative to the package root.
    fn toggle_package_resource(&self, item: &ResourceItem, enabled: bool) {
        let project = item.metadata.scope == SourceScope::Project;
        let mut settings = self.settings.borrow_mut();
        let source = if project {
            settings.project_settings()
        } else {
            settings.global_settings()
        };
        let mut packages: Vec<PackageSource> = source
            .get("packages")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let Some(index) = packages
            .iter()
            .position(|p| p.source() == item.metadata.source)
        else {
            return;
        };
        let mut filter = match packages[index].clone() {
            PackageSource::Source(source) => PackageFilter {
                source,
                ..PackageFilter::default()
            },
            PackageSource::Filtered(filter) => filter,
        };
        let base = item
            .metadata
            .base_dir
            .clone()
            .unwrap_or_else(|| node_path::dirname(&item.path));
        let pattern = node_path::relative(&base, &item.path);
        let slot = match item.resource_type {
            ResourceType::Extensions => &mut filter.extensions,
            ResourceType::Skills => &mut filter.skills,
            ResourceType::Prompts => &mut filter.prompts,
            ResourceType::Themes => &mut filter.themes,
        };
        let updated = Self::with_toggle(slot.as_deref().unwrap_or_default(), &pattern, enabled);
        *slot = (!updated.is_empty()).then_some(updated);
        let has_filters = filter.extensions.is_some()
            || filter.skills.is_some()
            || filter.prompts.is_some()
            || filter.themes.is_some();
        packages[index] = if has_filters {
            PackageSource::Filtered(filter)
        } else {
            PackageSource::Source(filter.source)
        };
        if project {
            settings.set_project_packages(&packages);
        } else {
            settings.set_packages(&packages);
        }
    }

    fn toggle_selected(&mut self) {
        let Some(&entry) = self.filtered.get(self.selected) else {
            return;
        };
        let Entry::Item(g, s, i) = entry else {
            return;
        };
        let item = self.groups[g].subgroups[s].items[i].clone();
        let enabled = !item.enabled;
        self.toggle_resource(&item, enabled);
        self.groups[g].subgroups[s].items[i].enabled = enabled;
        if let Some(cb) = &mut self.on_toggle {
            cb(&item, enabled);
        }
    }
}

impl Component for ResourceList {
    fn render(&mut self, width: u16) -> Vec<String> {
        let t = theme();
        let w = width as usize;
        let mut lines = self.search.render(width);
        lines.push(String::new());
        if self.filtered.is_empty() {
            lines.push(t.fg("muted", "  No resources found"));
            return lines;
        }
        let len = self.filtered.len();
        let start = (self.selected as isize - (self.max_visible / 2) as isize)
            .min(len as isize - self.max_visible as isize)
            .max(0) as usize;
        let end = (start + self.max_visible).min(len);
        for i in start..end {
            let selected = i == self.selected;
            match self.filtered[i] {
                Entry::Group(g) => {
                    let label = t.fg("accent", &t.bold(&self.groups[g].label));
                    lines.push(truncate_to_width(&format!("  {label}"), w, "", false));
                }
                Entry::Subgroup(g, s) => {
                    let label = t.fg("muted", &self.groups[g].subgroups[s].label);
                    lines.push(truncate_to_width(&format!("    {label}"), w, "", false));
                }
                entry @ Entry::Item(..) => {
                    let item = self.item(entry).unwrap();
                    let cursor = if selected {
                        t.fg("accent", SELECT_CURSOR)
                    } else {
                        select_gutter()
                    };
                    let checkbox = if item.enabled {
                        t.fg("success", "[x]")
                    } else {
                        t.fg("dim", "[ ]")
                    };
                    let name = if selected {
                        t.bold(&t.fg("accent", &item.display_name))
                    } else {
                        item.display_name.clone()
                    };
                    let line = truncate_to_width(
                        &format!("{cursor}    {checkbox} {name}"),
                        w,
                        "...",
                        false,
                    );
                    lines.push(if selected {
                        paint_selected_row(&line, w)
                    } else {
                        line
                    });
                }
            }
        }
        if start > 0 || end < len {
            let count = self.filtered.iter().filter(|e| Self::is_item(e)).count();
            let current = self.filtered[..self.selected]
                .iter()
                .filter(|e| Self::is_item(e))
                .count()
                + 1;
            lines.push(t.fg("dim", &format!("  ({current}/{count})")));
        }
        lines
    }

    fn handle_input(&mut self, data: &str) {
        let kb = get_keybindings();
        if kb.matches(data, "tui.select.up") {
            self.selected = self.find_next_item(self.selected, false);
        } else if kb.matches(data, "tui.select.down") {
            self.selected = self.find_next_item(self.selected, true);
        } else if kb.matches(data, "tui.select.pageUp") {
            // Up a page, then the nearest resource at or below it.
            let mut target = self.selected.saturating_sub(self.max_visible);
            while target < self.filtered.len() && !Self::is_item(&self.filtered[target]) {
                target += 1;
            }
            if target < self.filtered.len() {
                self.selected = target;
            }
        } else if kb.matches(data, "tui.select.pageDown") {
            // Down a page, then the nearest resource at or above it.
            let mut target = (self.selected + self.max_visible)
                .min(self.filtered.len().saturating_sub(1)) as isize;
            while target >= 0 && !Self::is_item(&self.filtered[target as usize]) {
                target -= 1;
            }
            if target >= 0 {
                self.selected = target as usize;
            }
        } else if kb.matches(data, "tui.select.cancel") {
            if let Some(cb) = &mut self.on_cancel {
                cb();
            }
        } else if matches_key(data, "ctrl+c") {
            if let Some(cb) = &mut self.on_exit {
                cb();
            }
        } else if data == " " || kb.matches(data, "tui.select.confirm") {
            self.toggle_selected();
        } else {
            self.search.handle_input_with(data, &kb);
            let query = self.search.get_value().to_string();
            self.filter_items(&query);
        }
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.search.set_focused(focused);
    }
}

/// `ConfigSelectorComponent`: header, then the resource list.
pub struct ConfigSelectorComponent {
    container: Container,
    list: Rc<RefCell<ResourceList>>,
}

impl ConfigSelectorComponent {
    /// `on_close` on escape, `on_exit` on ctrl+c.
    pub fn new(
        resolved: &ResolvedPaths,
        settings: Rc<RefCell<SettingsManager>>,
        cwd: &str,
        agent_dir: &str,
        on_close: impl FnMut() + 'static,
        on_exit: impl FnMut() + 'static,
    ) -> Self {
        let mut container = Container::default();
        // Minimal chrome: no filled borders.
        container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        container.add_child(Rc::new(RefCell::new(Header)));
        container.add_child(Rc::new(RefCell::new(Spacer::new(1))));
        let mut list = ResourceList::new(build_groups(resolved), settings, cwd, agent_dir);
        list.on_cancel = Some(Box::new(on_close));
        list.on_exit = Some(Box::new(on_exit));
        let list = Rc::new(RefCell::new(list));
        container.add_child(list.clone() as ComponentHandle);
        Self { container, list }
    }

    /// `getResourceList()`: what the keyboard goes to.
    pub fn resource_list(&self) -> Rc<RefCell<ResourceList>> {
        self.list.clone()
    }
}

impl Component for ConfigSelectorComponent {
    fn render(&mut self, width: u16) -> Vec<String> {
        self.container.render(width)
    }

    fn handle_input(&mut self, data: &str) {
        self.list.borrow_mut().handle_input(data);
    }

    fn invalidate(&mut self) {
        self.container.invalidate();
    }

    fn is_focusable(&self) -> bool {
        true
    }

    fn set_focused(&mut self, focused: bool) {
        self.list.borrow_mut().set_focused(focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_sort_matches_insertion_order_for_ties() {
        let mut v = vec![(1, 'a'), (0, 'b'), (1, 'c'), (0, 'd')];
        js_sort(&mut v, |a, b| a.0.cmp(&b.0));
        assert_eq!(v, [(0, 'b'), (0, 'd'), (1, 'a'), (1, 'c')]);
    }

    #[test]
    fn a_toggle_replaces_every_earlier_entry_for_the_pattern() {
        let current = vec!["-a.md".to_string(), "b.md".to_string(), "!a.md".to_string()];
        assert_eq!(
            ResourceList::with_toggle(&current, "a.md", true),
            ["b.md", "+a.md"]
        );
    }
}
