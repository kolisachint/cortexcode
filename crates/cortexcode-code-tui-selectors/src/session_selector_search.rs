//! Session search (`components/session-selector-search.ts`): fuzzy tokens,
//! `"quoted phrases"` and `re:<pattern>`, over a session's id, name, message
//! text and cwd.

use cortexcode_code_session::SessionInfo;
use cortexcode_tui_fuzzy::fuzzy_match;
use cortexcode_tui_util::js_regex::{byte_to_utf16, is_js_space, JsRegex};

/// How the list is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    Threaded,
    Recent,
    Relevance,
}

/// Which sessions the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameFilter {
    All,
    Named,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Fuzzy,
    Phrase,
}

enum ParsedQuery {
    Tokens(Vec<(TokenKind, String)>),
    /// `None` when the pattern did not compile (nothing matches).
    Regex(Option<JsRegex>),
}

struct MatchResult {
    matches: bool,
    /// Lower is better; only meaningful when `matches`.
    score: f64,
}

fn js_trim(text: &str) -> &str {
    text.trim_matches(is_js_space)
}

/// `text.toLowerCase().replace(/\s+/g, " ").trim()`.
fn normalize_whitespace_lower(text: &str) -> String {
    let lower = text.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut in_space = false;
    for c in lower.chars() {
        if is_js_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    js_trim(&out).to_string()
}

fn session_search_text(session: &SessionInfo) -> String {
    format!(
        "{} {} {} {}",
        session.id,
        session.name.as_deref().unwrap_or(""),
        session.all_messages_text,
        session.cwd
    )
}

/// `hasSessionName`: a name with something other than whitespace.
pub fn has_session_name(session: &SessionInfo) -> bool {
    session
        .name
        .as_deref()
        .is_some_and(|n| !js_trim(n).is_empty())
}

/// `None` when parsing failed (the query matches nothing).
fn parse_search_query(query: &str) -> Option<ParsedQuery> {
    let trimmed = js_trim(query);
    if trimmed.is_empty() {
        return Some(ParsedQuery::Tokens(Vec::new()));
    }

    if let Some(pattern) = trimmed.strip_prefix("re:") {
        let pattern = js_trim(pattern);
        if pattern.is_empty() {
            return None;
        }
        return JsRegex::try_new(pattern, "i")
            .ok()
            .map(|re| ParsedQuery::Regex(Some(re)));
    }

    // Tokens, with quoted phrases: foo "node cve" bar
    let mut tokens = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let flush = |buf: &mut String, tokens: &mut Vec<(TokenKind, String)>, kind| {
        let v = js_trim(buf).to_string();
        buf.clear();
        if !v.is_empty() {
            tokens.push((kind, v));
        }
    };
    for ch in trimmed.chars() {
        if ch == '"' {
            if in_quote {
                flush(&mut buf, &mut tokens, TokenKind::Phrase);
                in_quote = false;
            } else {
                flush(&mut buf, &mut tokens, TokenKind::Fuzzy);
                in_quote = true;
            }
            continue;
        }
        if !in_quote && is_js_space(ch) {
            flush(&mut buf, &mut tokens, TokenKind::Fuzzy);
            continue;
        }
        buf.push(ch);
    }

    // Unbalanced quotes: plain whitespace tokens.
    if in_quote {
        return Some(ParsedQuery::Tokens(
            trimmed
                .split(is_js_space)
                .filter(|t| !t.is_empty())
                .map(|t| (TokenKind::Fuzzy, t.to_string()))
                .collect(),
        ));
    }

    flush(&mut buf, &mut tokens, TokenKind::Fuzzy);
    Some(ParsedQuery::Tokens(tokens))
}

fn match_session(session: &SessionInfo, parsed: &ParsedQuery) -> MatchResult {
    let text = session_search_text(session);
    let no = MatchResult {
        matches: false,
        score: 0.0,
    };

    let tokens = match parsed {
        ParsedQuery::Regex(None) => return no,
        ParsedQuery::Regex(Some(re)) => {
            let idx = re.search(&text);
            if idx < 0 {
                return no;
            }
            return MatchResult {
                matches: true,
                score: idx as f64 * 0.1,
            };
        }
        ParsedQuery::Tokens(tokens) => tokens,
    };

    let mut total = 0.0;
    let mut normalized: Option<String> = None;
    for (kind, value) in tokens {
        match kind {
            TokenKind::Phrase => {
                let normalized =
                    normalized.get_or_insert_with(|| normalize_whitespace_lower(&text));
                let phrase = normalize_whitespace_lower(value);
                if phrase.is_empty() {
                    continue;
                }
                let Some(idx) = normalized.find(&phrase) else {
                    return no;
                };
                total += byte_to_utf16(normalized, idx) as f64 * 0.1;
            }
            TokenKind::Fuzzy => {
                let m = fuzzy_match(value, &text);
                if !m.matches {
                    return no;
                }
                total += m.score;
            }
        }
    }
    MatchResult {
        matches: true,
        score: total,
    }
}

/// `filterAndSortSessions`: the sessions matching `query`. `Recent` keeps the
/// incoming order; otherwise best score first, newer first on a tie.
pub fn filter_and_sort_sessions(
    sessions: &[SessionInfo],
    query: &str,
    sort_mode: SortMode,
    name_filter: NameFilter,
) -> Vec<SessionInfo> {
    let name_filtered: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| name_filter == NameFilter::All || has_session_name(s))
        .collect();
    if js_trim(query).is_empty() {
        return name_filtered.into_iter().cloned().collect();
    }

    let Some(parsed) = parse_search_query(query) else {
        return Vec::new();
    };

    if sort_mode == SortMode::Recent {
        return name_filtered
            .into_iter()
            .filter(|s| match_session(s, &parsed).matches)
            .cloned()
            .collect();
    }

    let mut scored: Vec<(&SessionInfo, f64)> = name_filtered
        .into_iter()
        .filter_map(|s| {
            let r = match_session(s, &parsed);
            r.matches.then_some((s, r.score))
        })
        .collect();
    scored.sort_by(|(a, sa), (b, sb)| {
        if sa != sb {
            return sa.partial_cmp(sb).unwrap_or(std::cmp::Ordering::Equal);
        }
        b.modified.cmp(&a.modified)
    });
    scored.into_iter().map(|(s, _)| s.clone()).collect()
}
