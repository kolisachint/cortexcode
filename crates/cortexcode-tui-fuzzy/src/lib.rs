//! Fuzzy matching for the cortex TUI.
//!
//! Matches if all query characters appear in order (not necessarily
//! consecutive). Lower score = better match.
//!
//! Ported from TypeScript `@kolisachint/hoocode-tui` → `fuzzy.ts`.

/// Result of a fuzzy-match attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FuzzyMatch {
    pub matches: bool,
    pub score: f64,
}

fn is_word_boundary_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '-' | '_' | '.' | '/' | ':')
}

fn match_query(normalized_query: &[char], text_lower: &[char]) -> FuzzyMatch {
    if normalized_query.is_empty() {
        return FuzzyMatch {
            matches: true,
            score: 0.0,
        };
    }
    if normalized_query.len() > text_lower.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    let mut query_index = 0usize;
    let mut score = 0.0f64;
    let mut last_match_index: i64 = -1;
    let mut consecutive_matches = 0i64;

    for (i, &ch) in text_lower.iter().enumerate() {
        if query_index >= normalized_query.len() {
            break;
        }
        if ch != normalized_query[query_index] {
            continue;
        }

        let is_word_boundary = i == 0 || is_word_boundary_char(text_lower[i - 1]);

        if last_match_index == i as i64 - 1 {
            consecutive_matches += 1;
            score -= consecutive_matches as f64 * 5.0;
        } else {
            consecutive_matches = 0;
            if last_match_index >= 0 {
                score += (i as i64 - last_match_index - 1) as f64 * 2.0;
            }
        }

        if is_word_boundary {
            score -= 10.0;
        }
        score += i as f64 * 0.1;

        last_match_index = i as i64;
        query_index += 1;
    }

    if query_index < normalized_query.len() {
        return FuzzyMatch {
            matches: false,
            score: 0.0,
        };
    }

    if normalized_query == text_lower {
        score -= 100.0;
    }

    FuzzyMatch {
        matches: true,
        score,
    }
}

/// Split a lowercase query into `(digits, letters)` if it's letters-then-digits
/// or digits-then-letters, so `"v2"` can also match text ordered as `"2v"`.
fn swapped_alnum_query(query_lower: &str) -> Option<String> {
    let chars: Vec<char> = query_lower.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let split = chars.iter().position(|c| !c.is_ascii_lowercase());
    match split {
        Some(idx) if idx > 0 => {
            let (letters, rest) = chars.split_at(idx);
            if rest.iter().all(|c| c.is_ascii_digit()) && !rest.is_empty() {
                let mut swapped: String = rest.iter().collect();
                swapped.extend(letters);
                return Some(swapped);
            }
            None
        }
        _ => {
            // Try digits-then-letters.
            let split = chars.iter().position(|c| !c.is_ascii_digit());
            match split {
                Some(idx) if idx > 0 => {
                    let (digits, rest) = chars.split_at(idx);
                    if rest.iter().all(|c| c.is_ascii_lowercase()) && !rest.is_empty() {
                        let mut swapped: String = rest.iter().collect();
                        swapped.extend(digits);
                        Some(swapped)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        }
    }
}

/// Fuzzy-match `query` against `text`. Lower score is a better match.
pub fn fuzzy_match(query: &str, text: &str) -> FuzzyMatch {
    let query_lower = query.to_lowercase();
    let text_lower = text.to_lowercase();
    let text_chars: Vec<char> = text_lower.chars().collect();

    let query_chars: Vec<char> = query_lower.chars().collect();
    let primary = match_query(&query_chars, &text_chars);
    if primary.matches {
        return primary;
    }

    let Some(swapped_query) = swapped_alnum_query(&query_lower) else {
        return primary;
    };

    let swapped_chars: Vec<char> = swapped_query.chars().collect();
    let swapped_match = match_query(&swapped_chars, &text_chars);
    if !swapped_match.matches {
        return primary;
    }

    FuzzyMatch {
        matches: true,
        score: swapped_match.score + 5.0,
    }
}

/// Filter and sort `items` by fuzzy-match quality against `query` (best
/// matches first). Space-separated query tokens must all match.
pub fn fuzzy_filter<T: Clone>(items: &[T], query: &str, get_text: impl Fn(&T) -> String) -> Vec<T> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return items.to_vec();
    }

    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return items.to_vec();
    }

    let mut results: Vec<(T, f64)> = Vec::new();
    for item in items {
        let text = get_text(item);
        let mut total_score = 0.0;
        let mut all_match = true;

        for token in &tokens {
            let m = fuzzy_match(token, &text);
            if m.matches {
                total_score += m.score;
            } else {
                all_match = false;
                break;
            }
        }

        if all_match {
            results.push((item.clone(), total_score));
        }
    }

    results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    results.into_iter().map(|(item, _)| item).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_match_empty_query_matches_everything() {
        let m = fuzzy_match("", "anything");
        assert!(m.matches);
        assert_eq!(m.score, 0.0);
    }

    #[test]
    fn test_fuzzy_match_query_longer_than_text_fails() {
        assert!(!fuzzy_match("abcdef", "abc").matches);
    }

    #[test]
    fn test_fuzzy_match_exact_match_is_best() {
        let exact = fuzzy_match("hello", "hello");
        let partial = fuzzy_match("hello", "hello world");
        assert!(exact.matches);
        assert!(partial.matches);
        assert!(
            exact.score < partial.score,
            "exact match should score lower (better)"
        );
    }

    #[test]
    fn test_fuzzy_match_out_of_order_fails() {
        assert!(!fuzzy_match("ba", "ab").matches);
    }

    #[test]
    fn test_fuzzy_match_non_consecutive_still_matches() {
        let m = fuzzy_match("fb", "foobar");
        assert!(m.matches);
    }

    #[test]
    fn test_fuzzy_match_word_boundary_scores_better() {
        // "gs" matches at word-boundary in "get_stuff" (g at start, s after _)
        // vs a case with no boundary alignment.
        let boundary = fuzzy_match("gs", "get_stuff");
        let no_boundary = fuzzy_match("gs", "xgxsx");
        assert!(boundary.matches && no_boundary.matches);
        assert!(boundary.score < no_boundary.score);
    }

    #[test]
    fn test_fuzzy_match_case_insensitive() {
        assert!(fuzzy_match("HELLO", "hello world").matches);
    }

    #[test]
    fn test_fuzzy_match_swapped_alpha_digit() {
        // "2v" should still match text ordered "v2" via the swap fallback.
        let m = fuzzy_match("2v", "v2-model");
        assert!(m.matches);
    }

    #[test]
    fn test_fuzzy_filter_empty_query_returns_all() {
        let items = vec!["a", "b", "c"];
        let result = fuzzy_filter(&items, "", |s| s.to_string());
        assert_eq!(result, items);
    }

    #[test]
    fn test_fuzzy_filter_sorts_best_matches_first() {
        let items = vec!["xylophone", "hello", "help"];
        let result = fuzzy_filter(&items, "hel", |s| s.to_string());
        assert_eq!(result, vec!["hello", "help"]);
    }

    #[test]
    fn test_fuzzy_filter_multi_token_requires_all_match() {
        let items = vec!["foo bar", "foo baz", "qux"];
        let result = fuzzy_filter(&items, "foo bar", |s| s.to_string());
        assert_eq!(result, vec!["foo bar"]);
    }

    #[test]
    fn test_fuzzy_filter_excludes_non_matches() {
        let items = vec!["apple", "banana"];
        let result = fuzzy_filter(&items, "xyz", |s| s.to_string());
        assert!(result.is_empty());
    }
}
