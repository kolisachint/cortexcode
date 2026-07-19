//! Slash-command / file-path autocompletion, ported from `autocomplete.ts`.
//!
//! Adaptation: the TypeScript original is async (`Promise`-returning
//! `getSuggestions`, `AbortSignal`-cancellable `fd` subprocess spawning).
//! No async runtime is wired into this crate, so [`AutocompleteProvider`]
//! is synchronous: suggestion generation (a directory read or an `fd`
//! subprocess call) blocks the calling thread. `fd` invocations here
//! typically complete in single-digit milliseconds, and callers that need
//! cancellation can run this behind their own timeout/thread if needed —
//! but true mid-flight process cancellation via `AbortSignal` is not
//! ported.

mod path_utils;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cortexcode_tui_fuzzy::fuzzy_filter;

use path_utils::{
    basename, build_completion_value, build_fd_path_query, dirname, expand_home_path,
    extract_quoted_prefix, find_last_delimiter, is_dir, join_path, parse_path_prefix,
    to_display_path, CompletionValueOptions,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: Option<String>,
}

pub type ArgumentCompletionsFn = Box<dyn Fn(&str) -> Option<Vec<AutocompleteItem>>>;

pub struct SlashCommand {
    pub name: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    /// Returns `None` if no argument completion is available for this command.
    pub get_argument_completions: Option<ArgumentCompletionsFn>,
}

/// A registered command: either a full [`SlashCommand`] (with optional
/// argument completion) or a plain [`AutocompleteItem`] shorthand.
pub enum CommandEntry {
    Slash(SlashCommand),
    Item(AutocompleteItem),
}

impl CommandEntry {
    fn name(&self) -> &str {
        match self {
            CommandEntry::Slash(cmd) => &cmd.name,
            CommandEntry::Item(item) => &item.value,
        }
    }

    fn description(&self) -> Option<&str> {
        match self {
            CommandEntry::Slash(cmd) => cmd.description.as_deref(),
            CommandEntry::Item(item) => item.description.as_deref(),
        }
    }

    fn argument_hint(&self) -> Option<&str> {
        match self {
            CommandEntry::Slash(cmd) => cmd.argument_hint.as_deref(),
            CommandEntry::Item(_) => None,
        }
    }
}

pub struct AutocompleteSuggestions {
    pub items: Vec<AutocompleteItem>,
    pub prefix: String,
}

pub struct ApplyCompletionResult {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
}

pub trait AutocompleteProvider {
    /// Get autocomplete suggestions for the current text/cursor position.
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions>;

    /// Apply the selected item, returning the new text and cursor position.
    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> ApplyCompletionResult;

    /// Whether file completion should trigger for explicit Tab completion.
    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let _ = (lines, cursor_line, cursor_col);
        true
    }
}

/// Combined provider that handles both slash commands and file paths.
pub struct CombinedAutocompleteProvider {
    commands: Vec<CommandEntry>,
    base_path: PathBuf,
    fd_path: Option<PathBuf>,
}

fn cursor_line_text(lines: &[String], cursor_line: usize, cursor_col: usize) -> String {
    let line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
    let byte_end = char_index_to_byte(line, cursor_col);
    line[..byte_end].to_string()
}

fn char_index_to_byte(s: &str, char_index: usize) -> usize {
    s.char_indices()
        .nth(char_index)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

impl CombinedAutocompleteProvider {
    pub fn new(
        commands: Vec<CommandEntry>,
        base_path: impl Into<PathBuf>,
        fd_path: Option<PathBuf>,
    ) -> Self {
        Self {
            commands,
            base_path: base_path.into(),
            fd_path,
        }
    }

    fn extract_at_prefix(&self, text: &str) -> Option<String> {
        if let Some(quoted) = extract_quoted_prefix(text) {
            if quoted.starts_with("@\"") {
                return Some(quoted);
            }
        }

        let chars: Vec<char> = text.chars().collect();
        let token_start = find_last_delimiter(text).map(|i| i + 1).unwrap_or(0);

        if chars.get(token_start) == Some(&'@') {
            return Some(chars[token_start..].iter().collect());
        }
        None
    }

    fn extract_path_prefix(&self, text: &str, force_extract: bool) -> Option<String> {
        if let Some(quoted) = extract_quoted_prefix(text) {
            return Some(quoted);
        }

        let chars: Vec<char> = text.chars().collect();
        let last_delim = find_last_delimiter(text);
        let path_prefix: String = match last_delim {
            Some(idx) => chars[idx + 1..].iter().collect(),
            None => text.to_string(),
        };

        if force_extract {
            return Some(path_prefix);
        }

        if path_prefix.contains('/')
            || path_prefix.starts_with('.')
            || path_prefix.starts_with("~/")
        {
            return Some(path_prefix);
        }

        if path_prefix.is_empty() && text.ends_with(' ') {
            return Some(path_prefix);
        }

        None
    }

    fn resolve_search_dir(
        &self,
        raw_prefix: &str,
        expanded_prefix: &str,
        dir_part: &str,
    ) -> PathBuf {
        if raw_prefix.starts_with('~') || expanded_prefix.starts_with('/') {
            PathBuf::from(dir_part)
        } else {
            PathBuf::from(join_path(&self.base_path.to_string_lossy(), dir_part))
        }
    }

    fn get_file_suggestions(&self, prefix: &str) -> Vec<AutocompleteItem> {
        let parsed = parse_path_prefix(prefix);
        let raw_prefix = parsed.raw_prefix.as_str();
        let mut expanded_prefix = raw_prefix.to_string();
        if expanded_prefix.starts_with('~') {
            expanded_prefix = expand_home_path(&expanded_prefix);
        }

        let is_root_prefix = matches!(raw_prefix, "" | "./" | "../" | "~" | "~/" | "/")
            || (parsed.is_at_prefix && raw_prefix.is_empty());

        let (search_dir, search_prefix): (PathBuf, String) =
            if is_root_prefix || raw_prefix.ends_with('/') {
                (
                    self.resolve_search_dir(raw_prefix, &expanded_prefix, &expanded_prefix),
                    String::new(),
                )
            } else {
                let dir = dirname(&expanded_prefix);
                let file = basename(&expanded_prefix);
                (
                    self.resolve_search_dir(raw_prefix, &expanded_prefix, &dir),
                    file,
                )
            };

        let Ok(entries) = fs::read_dir(&search_dir) else {
            return Vec::new();
        };

        let mut suggestions = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name
                .to_lowercase()
                .starts_with(&search_prefix.to_lowercase())
            {
                continue;
            }

            let file_type = entry.file_type();
            let mut is_directory = file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false);
            if !is_directory && file_type.as_ref().map(|t| t.is_symlink()).unwrap_or(false) {
                is_directory = is_dir(&entry.path());
            }

            let display_prefix = raw_prefix;
            let relative_path = if display_prefix.ends_with('/') {
                format!("{display_prefix}{name}")
            } else if display_prefix.contains('/') || display_prefix.contains('\\') {
                if let Some(home_rel) = display_prefix.strip_prefix("~/") {
                    let dir = dirname(home_rel);
                    if dir == "." {
                        format!("~/{name}")
                    } else {
                        format!("~/{}", join_path(&dir, &name))
                    }
                } else if let Some(_abs) = display_prefix.strip_prefix('/') {
                    let dir = dirname(display_prefix);
                    if dir == "/" {
                        format!("/{name}")
                    } else {
                        format!("{dir}/{name}")
                    }
                } else {
                    let dir = dirname(display_prefix);
                    let mut joined = join_path(&dir, &name);
                    if display_prefix.starts_with("./") && !joined.starts_with("./") {
                        joined = format!("./{joined}");
                    }
                    joined
                }
            } else if display_prefix.starts_with('~') {
                format!("~/{name}")
            } else {
                name.clone()
            };

            let relative_path = to_display_path(&relative_path);
            let path_value = if is_directory {
                format!("{relative_path}/")
            } else {
                relative_path
            };
            let value = build_completion_value(
                &path_value,
                &CompletionValueOptions {
                    is_directory,
                    is_at_prefix: parsed.is_at_prefix,
                    is_quoted_prefix: parsed.is_quoted_prefix,
                },
            );

            suggestions.push(AutocompleteItem {
                value,
                label: if is_directory {
                    format!("{name}/")
                } else {
                    name
                },
                description: None,
            });
        }

        suggestions.sort_by(|a, b| {
            let a_is_dir = a.value.ends_with('/');
            let b_is_dir = b.value.ends_with('/');
            match (a_is_dir, b_is_dir) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.label.cmp(&b.label),
            }
        });

        suggestions
    }

    fn score_entry(file_path: &str, query: &str, is_directory: bool) -> i32 {
        let file_name = basename(file_path).to_lowercase();
        let lower_query = query.to_lowercase();

        let mut score = if file_name == lower_query {
            100
        } else if file_name.starts_with(&lower_query) {
            80
        } else if file_name.contains(&lower_query) {
            50
        } else if file_path.to_lowercase().contains(&lower_query) {
            30
        } else {
            0
        };

        if is_directory && score > 0 {
            score += 10;
        }
        score
    }

    fn resolve_scoped_fuzzy_query(&self, raw_query: &str) -> Option<(PathBuf, String, String)> {
        let normalized_query = to_display_path(raw_query);
        let slash_index = normalized_query.rfind('/')?;
        let display_base = normalized_query[..slash_index + 1].to_string();
        let query = normalized_query[slash_index + 1..].to_string();

        let base_dir = if display_base.starts_with("~/") {
            PathBuf::from(expand_home_path(&display_base))
        } else if display_base.starts_with('/') {
            PathBuf::from(&display_base)
        } else {
            PathBuf::from(join_path(&self.base_path.to_string_lossy(), &display_base))
        };

        if !is_dir(&base_dir) {
            return None;
        }

        Some((base_dir, query, display_base))
    }

    fn scoped_path_for_display(display_base: &str, relative_path: &str) -> String {
        let normalized = to_display_path(relative_path);
        if display_base == "/" {
            format!("/{normalized}")
        } else {
            format!("{}{normalized}", to_display_path(display_base))
        }
    }

    /// Fuzzy file search using `fd` (fast, respects `.gitignore`). Blocking:
    /// see the module-level docs on the async→sync adaptation.
    fn get_fuzzy_file_suggestions(
        &self,
        query: &str,
        is_quoted_prefix: bool,
    ) -> Vec<AutocompleteItem> {
        let Some(fd_path) = &self.fd_path else {
            return Vec::new();
        };

        let scoped = self.resolve_scoped_fuzzy_query(query);
        let fd_base_dir = scoped
            .as_ref()
            .map(|(dir, _, _)| dir.clone())
            .unwrap_or_else(|| self.base_path.clone());
        let fd_query = scoped
            .as_ref()
            .map(|(_, q, _)| q.clone())
            .unwrap_or_else(|| query.to_string());

        let entries = walk_directory_with_fd(&fd_base_dir, fd_path, &fd_query, 100);

        let mut scored: Vec<(FdEntry, i32)> = entries
            .into_iter()
            .map(|entry| {
                let score = if fd_query.is_empty() {
                    1
                } else {
                    Self::score_entry(&entry.path, &fd_query, entry.is_directory)
                };
                (entry, score)
            })
            .filter(|(_, score)| *score > 0)
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.truncate(20);

        let mut suggestions = Vec::new();
        for (entry, _) in scored {
            let path_without_slash = if entry.is_directory {
                entry.path.trim_end_matches('/').to_string()
            } else {
                entry.path.clone()
            };
            let display_path = match &scoped {
                Some((_, _, display_base)) => {
                    Self::scoped_path_for_display(display_base, &path_without_slash)
                }
                None => path_without_slash.clone(),
            };
            let entry_name = basename(&path_without_slash);
            let completion_path = if entry.is_directory {
                format!("{display_path}/")
            } else {
                display_path.clone()
            };
            let value = build_completion_value(
                &completion_path,
                &CompletionValueOptions {
                    is_directory: entry.is_directory,
                    is_at_prefix: true,
                    is_quoted_prefix,
                },
            );

            suggestions.push(AutocompleteItem {
                value,
                label: if entry.is_directory {
                    format!("{entry_name}/")
                } else {
                    entry_name
                },
                description: Some(display_path),
            });
        }

        suggestions
    }
}

struct FdEntry {
    path: String,
    is_directory: bool,
}

fn walk_directory_with_fd(
    base_dir: &Path,
    fd_path: &Path,
    query: &str,
    max_results: usize,
) -> Vec<FdEntry> {
    let mut args: Vec<String> = vec![
        "--base-directory".into(),
        base_dir.to_string_lossy().into_owned(),
        "--max-results".into(),
        max_results.to_string(),
        "--type".into(),
        "f".into(),
        "--type".into(),
        "d".into(),
        "--follow".into(),
        "--hidden".into(),
        "--exclude".into(),
        ".git".into(),
        "--exclude".into(),
        ".git/*".into(),
        "--exclude".into(),
        ".git/**".into(),
    ];

    if to_display_path(query).contains('/') {
        args.push("--full-path".into());
    }

    if !query.is_empty() {
        args.push(build_fd_path_query(query));
    }

    let Ok(output) = Command::new(fd_path).args(&args).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Vec::new();
    }

    let mut results = Vec::new();
    for line in stdout.trim().lines() {
        if line.is_empty() {
            continue;
        }
        let display_line = to_display_path(line);
        let has_trailing_separator = display_line.ends_with('/');
        let normalized_path = if has_trailing_separator {
            display_line.trim_end_matches('/').to_string()
        } else {
            display_line.clone()
        };
        if normalized_path == ".git"
            || normalized_path.starts_with(".git/")
            || normalized_path.contains("/.git/")
        {
            continue;
        }
        results.push(FdEntry {
            path: display_line,
            is_directory: has_trailing_separator,
        });
    }
    results
}

impl AutocompleteProvider for CombinedAutocompleteProvider {
    fn get_suggestions(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        force: bool,
    ) -> Option<AutocompleteSuggestions> {
        let text_before_cursor = cursor_line_text(lines, cursor_line, cursor_col);

        if let Some(at_prefix) = self.extract_at_prefix(&text_before_cursor) {
            let parsed = parse_path_prefix(&at_prefix);
            let suggestions =
                self.get_fuzzy_file_suggestions(&parsed.raw_prefix, parsed.is_quoted_prefix);
            if suggestions.is_empty() {
                return None;
            }
            return Some(AutocompleteSuggestions {
                items: suggestions,
                prefix: at_prefix,
            });
        }

        if !force && text_before_cursor.starts_with('/') {
            let space_index = text_before_cursor.find(' ');

            if space_index.is_none() {
                let prefix = &text_before_cursor[1..];
                #[derive(Clone)]
                struct CmdView {
                    name: String,
                    label: String,
                    description: Option<String>,
                }
                let command_items: Vec<CmdView> = self
                    .commands
                    .iter()
                    .map(|cmd| {
                        let name = cmd.name().to_string();
                        let hint = cmd.argument_hint();
                        let desc = cmd.description().unwrap_or("");
                        let full_desc = match hint {
                            Some(h) if !desc.is_empty() => Some(format!("{h} — {desc}")),
                            Some(h) => Some(h.to_string()),
                            None if !desc.is_empty() => Some(desc.to_string()),
                            None => None,
                        };
                        CmdView {
                            name: name.clone(),
                            label: name,
                            description: full_desc,
                        }
                    })
                    .collect();

                let filtered = fuzzy_filter(&command_items, prefix, |item| item.name.clone());
                if filtered.is_empty() {
                    return None;
                }

                return Some(AutocompleteSuggestions {
                    items: filtered
                        .into_iter()
                        .map(|item| AutocompleteItem {
                            value: item.name,
                            label: item.label,
                            description: item.description,
                        })
                        .collect(),
                    prefix: text_before_cursor.clone(),
                });
            }

            let space_index = space_index.unwrap();
            let command_name = &text_before_cursor[1..space_index];
            let argument_text = &text_before_cursor[space_index + 1..];

            let command = self
                .commands
                .iter()
                .find(|cmd| cmd.name() == command_name)?;
            let CommandEntry::Slash(slash) = command else {
                return None;
            };
            let get_args = slash.get_argument_completions.as_ref()?;
            let argument_suggestions = get_args(argument_text)?;
            if argument_suggestions.is_empty() {
                return None;
            }

            return Some(AutocompleteSuggestions {
                items: argument_suggestions,
                prefix: argument_text.to_string(),
            });
        }

        let path_match = self.extract_path_prefix(&text_before_cursor, force)?;
        let suggestions = self.get_file_suggestions(&path_match);
        if suggestions.is_empty() {
            return None;
        }
        Some(AutocompleteSuggestions {
            items: suggestions,
            prefix: path_match,
        })
    }

    fn apply_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
        item: &AutocompleteItem,
        prefix: &str,
    ) -> ApplyCompletionResult {
        let current_line = lines.get(cursor_line).map(String::as_str).unwrap_or("");
        let current_chars: Vec<char> = current_line.chars().collect();
        let prefix_len = prefix.chars().count();
        let before_prefix_end = cursor_col.saturating_sub(prefix_len);
        let before_prefix: String = current_chars[..before_prefix_end.min(current_chars.len())]
            .iter()
            .collect();
        let after_cursor: String = current_chars[cursor_col.min(current_chars.len())..]
            .iter()
            .collect();

        let is_quoted_prefix = prefix.starts_with('"') || prefix.starts_with("@\"");
        let has_leading_quote_after_cursor = after_cursor.starts_with('"');
        let has_trailing_quote_in_item = item.value.ends_with('"');
        let adjusted_after_cursor =
            if is_quoted_prefix && has_trailing_quote_in_item && has_leading_quote_after_cursor {
                after_cursor.chars().skip(1).collect::<String>()
            } else {
                after_cursor.clone()
            };

        let is_slash_command = prefix.starts_with('/')
            && before_prefix.trim().is_empty()
            && !prefix[1..].contains('/');
        if is_slash_command {
            let final_line = format!("{before_prefix}/{} {adjusted_after_cursor}", item.value);
            let mut new_lines = lines.to_vec();
            if let Some(l) = new_lines.get_mut(cursor_line) {
                *l = final_line;
            }
            let cursor_col = before_prefix.chars().count() + item.value.chars().count() + 2;
            return ApplyCompletionResult {
                lines: new_lines,
                cursor_line,
                cursor_col,
            };
        }

        if prefix.starts_with('@') {
            let is_directory = item.label.ends_with('/');
            let suffix = if is_directory { "" } else { " " };
            let new_line = format!(
                "{before_prefix}{}{suffix}{adjusted_after_cursor}",
                item.value
            );
            let mut new_lines = lines.to_vec();
            if let Some(l) = new_lines.get_mut(cursor_line) {
                *l = new_line;
            }
            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.chars().count() - 1
            } else {
                item.value.chars().count()
            };
            let cursor_col = before_prefix.chars().count() + cursor_offset + suffix.chars().count();
            return ApplyCompletionResult {
                lines: new_lines,
                cursor_line,
                cursor_col,
            };
        }

        let text_before_cursor = cursor_line_text(lines, cursor_line, cursor_col);
        if text_before_cursor.contains('/') && text_before_cursor.contains(' ') {
            let new_line = format!("{before_prefix}{}{adjusted_after_cursor}", item.value);
            let mut new_lines = lines.to_vec();
            if let Some(l) = new_lines.get_mut(cursor_line) {
                *l = new_line;
            }
            let is_directory = item.label.ends_with('/');
            let has_trailing_quote = item.value.ends_with('"');
            let cursor_offset = if is_directory && has_trailing_quote {
                item.value.chars().count() - 1
            } else {
                item.value.chars().count()
            };
            let cursor_col = before_prefix.chars().count() + cursor_offset;
            return ApplyCompletionResult {
                lines: new_lines,
                cursor_line,
                cursor_col,
            };
        }

        let new_line = format!("{before_prefix}{}{adjusted_after_cursor}", item.value);
        let mut new_lines = lines.to_vec();
        if let Some(l) = new_lines.get_mut(cursor_line) {
            *l = new_line;
        }
        let is_directory = item.label.ends_with('/');
        let has_trailing_quote = item.value.ends_with('"');
        let cursor_offset = if is_directory && has_trailing_quote {
            item.value.chars().count() - 1
        } else {
            item.value.chars().count()
        };
        let cursor_col = before_prefix.chars().count() + cursor_offset;
        ApplyCompletionResult {
            lines: new_lines,
            cursor_line,
            cursor_col,
        }
    }

    fn should_trigger_file_completion(
        &self,
        lines: &[String],
        cursor_line: usize,
        cursor_col: usize,
    ) -> bool {
        let text_before_cursor = cursor_line_text(lines, cursor_line, cursor_col);
        let trimmed = text_before_cursor.trim();
        if trimmed.starts_with('/') && !trimmed.contains(' ') {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base: &Path) -> CombinedAutocompleteProvider {
        CombinedAutocompleteProvider::new(vec![], base, None)
    }

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), "").unwrap();
    }

    #[test]
    fn get_file_suggestions_lists_directory_contents() {
        let tmp = tempdir();
        touch(&tmp, "alpha.txt");
        touch(&tmp, "beta.txt");
        fs::create_dir(tmp.join("sub")).unwrap();

        let provider = provider(&tmp);
        let mut items = provider.get_file_suggestions("");
        items.sort_by(|a, b| a.label.cmp(&b.label));
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"alpha.txt"));
        assert!(labels.contains(&"beta.txt"));
        assert!(labels.contains(&"sub/"));
        cleanup(&tmp);
    }

    #[test]
    fn get_file_suggestions_filters_by_prefix() {
        let tmp = tempdir();
        touch(&tmp, "alpha.txt");
        touch(&tmp, "beta.txt");

        let provider = provider(&tmp);
        let items = provider.get_file_suggestions("al");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "alpha.txt");
        cleanup(&tmp);
    }

    #[test]
    fn directories_sort_before_files() {
        let tmp = tempdir();
        touch(&tmp, "afile.txt");
        fs::create_dir(tmp.join("zdir")).unwrap();

        let provider = provider(&tmp);
        let items = provider.get_file_suggestions("");
        assert_eq!(items[0].label, "zdir/");
        cleanup(&tmp);
    }

    #[test]
    fn apply_completion_slash_command_adds_space() {
        let provider = provider(Path::new("/tmp"));
        let lines = vec!["/hel".to_string()];
        let item = AutocompleteItem {
            value: "help".to_string(),
            label: "help".to_string(),
            description: None,
        };
        let result = provider.apply_completion(&lines, 0, 4, &item, "/hel");
        assert_eq!(result.lines[0], "/help ");
        assert_eq!(result.cursor_col, 6);
    }

    #[test]
    fn apply_completion_at_prefix_directory_has_no_trailing_space() {
        let provider = provider(Path::new("/tmp"));
        let lines = vec!["@sr".to_string()];
        let item = AutocompleteItem {
            value: "@src/".to_string(),
            label: "src/".to_string(),
            description: None,
        };
        let result = provider.apply_completion(&lines, 0, 3, &item, "@sr");
        assert_eq!(result.lines[0], "@src/");
        assert_eq!(result.cursor_col, 5);
    }

    #[test]
    fn should_trigger_file_completion_false_for_bare_slash_command() {
        let provider = provider(Path::new("/tmp"));
        let lines = vec!["/hel".to_string()];
        assert!(!provider.should_trigger_file_completion(&lines, 0, 4));
    }

    #[test]
    fn should_trigger_file_completion_true_after_space() {
        let provider = provider(Path::new("/tmp"));
        let lines = vec!["/help foo".to_string()];
        assert!(provider.should_trigger_file_completion(&lines, 0, 9));
    }

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!(
                "cortexcode-autocomplete-test-{}",
                std::process::id()
            ))
            .join(format!("{:x}", rand_seed()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn rand_seed() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }
}
