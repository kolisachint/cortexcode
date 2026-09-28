//! `diffWords` from the pinned jsdiff (8.0): its Myers core (`diff/base.js`,
//! with the diagonal pruning), the word tokenizer and the whitespace
//! post-processing (`diff/word.js`, `util/string.js`), ported literally so
//! the edit view highlights the same spans as the pin.

/// One change object: `value`, and whether it was added or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub value: String,
    pub added: bool,
    pub removed: bool,
    pub count: usize,
}

/// JavaScript's `\s`.
fn is_ws(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `extendedWordChars`.
fn is_word_char(c: char) -> bool {
    matches!(c,
        'a'..='z' | 'A'..='Z' | '0'..='9' | '_'
        | '\u{AD}'
        | '\u{C0}'..='\u{D6}'
        | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2C6}'
        | '\u{2C8}'..='\u{2D7}'
        | '\u{2DE}'..='\u{2FF}'
        | '\u{1E00}'..='\u{1EFF}')
}

/// `tokenizeIncludingWhitespace`: word runs, whitespace runs, single others.
fn raw_parts(value: &str) -> Vec<String> {
    let chars: Vec<char> = value.chars().collect();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        if is_word_char(c) {
            while j < chars.len() && is_word_char(chars[j]) {
                j += 1;
            }
        } else if is_ws(c) {
            while j < chars.len() && is_ws(chars[j]) {
                j += 1;
            }
        }
        parts.push(chars[i..j].iter().collect());
        i = j;
    }
    parts
}

fn has_ws(s: &str) -> bool {
    s.chars().any(is_ws)
}

/// `WordDiff.tokenize`: whitespace stitched onto the neighbouring tokens.
fn tokenize(value: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut prev: Option<String> = None;
    for part in raw_parts(value) {
        if has_ws(&part) {
            match prev {
                None => tokens.push(part.clone()),
                Some(_) => {
                    let last = tokens.pop().unwrap_or_default();
                    tokens.push(last + &part);
                }
            }
        } else if let Some(p) = prev.as_ref().filter(|p| has_ws(p)) {
            if tokens.last() == Some(p) {
                let last = tokens.pop().unwrap();
                tokens.push(last + &part);
            } else {
                tokens.push(format!("{p}{part}"));
            }
        } else {
            tokens.push(part.clone());
        }
        prev = Some(part);
    }
    tokens
}

fn js_trim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

fn equals(left: &str, right: &str) -> bool {
    js_trim(left) == js_trim(right)
}

/// `WordDiff.join`: leading whitespace off all but the first token.
fn join(tokens: &[String]) -> String {
    tokens
        .iter()
        .enumerate()
        .map(|(i, t)| {
            if i == 0 {
                t.as_str()
            } else {
                t.trim_start_matches(is_ws)
            }
        })
        .collect()
}

#[derive(Clone)]
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<Box<Component>>,
}

#[derive(Clone)]
struct Path {
    old_pos: isize,
    last: Option<Box<Component>>,
}

fn add_to_path(path: &Path, added: bool, removed: bool, old_pos_inc: isize) -> Path {
    match &path.last {
        Some(last) if last.added == added && last.removed == removed => Path {
            old_pos: path.old_pos + old_pos_inc,
            last: Some(Box::new(Component {
                count: last.count + 1,
                added,
                removed,
                previous: last.previous.clone(),
            })),
        },
        last => Path {
            old_pos: path.old_pos + old_pos_inc,
            last: Some(Box::new(Component {
                count: 1,
                added,
                removed,
                previous: last.clone(),
            })),
        },
    }
}

fn extract_common(path: &mut Path, new: &[String], old: &[String], diagonal: isize) -> isize {
    let (new_len, old_len) = (new.len() as isize, old.len() as isize);
    let mut old_pos = path.old_pos;
    let mut new_pos = old_pos - diagonal;
    let mut common = 0;
    while new_pos + 1 < new_len
        && old_pos + 1 < old_len
        && equals(&old[(old_pos + 1) as usize], &new[(new_pos + 1) as usize])
    {
        new_pos += 1;
        old_pos += 1;
        common += 1;
    }
    if common > 0 {
        path.last = Some(Box::new(Component {
            count: common,
            added: false,
            removed: false,
            previous: path.last.take(),
        }));
    }
    path.old_pos = old_pos;
    new_pos
}

fn build_values(last: Option<Box<Component>>, new: &[String], old: &[String]) -> Vec<Change> {
    let mut components = Vec::new();
    let mut next = last;
    while let Some(mut c) = next {
        next = c.previous.take();
        components.push(*c);
    }
    components.reverse();
    let (mut new_pos, mut old_pos) = (0, 0);
    components
        .into_iter()
        .map(|c| {
            let value = if !c.removed {
                let v = join(&new[new_pos..new_pos + c.count]);
                new_pos += c.count;
                if !c.added {
                    old_pos += c.count;
                }
                v
            } else {
                let v = join(&old[old_pos..old_pos + c.count]);
                old_pos += c.count;
                v
            };
            Change {
                value,
                added: c.added,
                removed: c.removed,
                count: c.count,
            }
        })
        .collect()
}

/// `Diff.diff` over tokens (no callback, no limits).
fn diff_tokens(old: &[String], new: &[String]) -> Vec<Change> {
    let (new_len, old_len) = (new.len() as isize, old.len() as isize);
    let max_edit_length = new_len + old_len;
    let offset = max_edit_length + 1;
    let mut best: Vec<Option<Path>> = vec![None; (2 * offset + 1) as usize];
    let at = |d: isize| (d + offset) as usize;
    let mut first = Path {
        old_pos: -1,
        last: None,
    };
    let new_pos = extract_common(&mut first, new, old, 0);
    if first.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build_values(first.last, new, old);
    }
    best[at(0)] = Some(first);
    let mut min_diagonal = isize::MIN;
    let mut max_diagonal = isize::MAX;
    let mut edit_length: isize = 1;
    while edit_length <= max_edit_length {
        let mut diagonal = min_diagonal.max(-edit_length);
        let top = max_diagonal.min(edit_length);
        while diagonal <= top {
            let remove_path = best[at(diagonal - 1)].take();
            let add_path = best[at(diagonal + 1)].clone();
            let can_add = add_path.as_ref().is_some_and(|p| {
                let add_new_pos = p.old_pos - diagonal;
                0 <= add_new_pos && add_new_pos < new_len
            });
            let can_remove = remove_path
                .as_ref()
                .is_some_and(|p| p.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best[at(diagonal)] = None;
                diagonal += 2;
                continue;
            }
            let mut base = if !can_remove
                || (can_add
                    && remove_path.as_ref().unwrap().old_pos < add_path.as_ref().unwrap().old_pos)
            {
                add_to_path(add_path.as_ref().unwrap(), true, false, 0)
            } else {
                add_to_path(remove_path.as_ref().unwrap(), false, true, 1)
            };
            let new_pos = extract_common(&mut base, new, old, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build_values(base.last, new, old);
            }
            if base.old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            best[at(diagonal)] = Some(base);
            diagonal += 2;
        }
        edit_length += 1;
    }
    Vec::new()
}

fn leading_ws(s: &str) -> &str {
    let end = s.find(|c: char| !is_ws(c)).unwrap_or(s.len());
    &s[..end]
}

fn trailing_ws(s: &str) -> &str {
    let start = s
        .char_indices()
        .rev()
        .find(|(_, c)| !is_ws(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &s[start..]
}

fn longest_common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut end = 0;
    for ((i, ca), cb) in a.char_indices().zip(b.chars()) {
        if ca != cb {
            return &a[..i];
        }
        end = i + ca.len_utf8();
    }
    &a[..end]
}

fn longest_common_suffix<'a>(a: &'a str, b: &str) -> &'a str {
    let mut start = a.len();
    for ((i, ca), cb) in a.char_indices().rev().zip(b.chars().rev()) {
        if ca != cb {
            break;
        }
        start = i;
    }
    &a[start..]
}

fn replace_prefix(s: &str, old: &str, new: &str) -> String {
    debug_assert!(s.starts_with(old), "{s:?} doesn't start with {old:?}");
    format!("{new}{}", s.get(old.len()..).unwrap_or(""))
}

fn replace_suffix(s: &str, old: &str, new: &str) -> String {
    if old.is_empty() {
        return format!("{s}{new}");
    }
    debug_assert!(s.ends_with(old), "{s:?} doesn't end with {old:?}");
    format!("{}{new}", &s[..s.len().saturating_sub(old.len())])
}

/// `maximumOverlap`: the longest prefix of `b` that is a suffix of `a`.
fn maximum_overlap<'a>(a: &str, b: &'a str) -> &'a str {
    let a: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    let start_a = a.len().saturating_sub(bc.len());
    let end_b = bc.len().min(a.len());
    if end_b == 0 {
        return "";
    }
    let mut map = vec![0usize; end_b];
    let mut k = 0;
    for j in 1..end_b {
        map[j] = if bc[j] == bc[k] { map[k] } else { k };
        while k > 0 && bc[j] != bc[k] {
            k = map[k];
        }
        if bc[j] == bc[k] {
            k += 1;
        }
    }
    k = 0;
    for &ca in &a[start_a..] {
        while k > 0 && ca != bc[k] {
            k = map[k];
        }
        if k < bc.len() && ca == bc[k] {
            k += 1;
        }
    }
    let byte_end: usize = bc[..k].iter().map(|c| c.len_utf8()).sum();
    &b[..byte_end]
}

fn dedupe_whitespace(
    changes: &mut [Change],
    start: Option<usize>,
    del: Option<usize>,
    ins: Option<usize>,
    end: Option<usize>,
) {
    match (del, ins) {
        (Some(d), Some(i)) => {
            let old_prefix = leading_ws(&changes[d].value).to_string();
            let old_suffix = trailing_ws(&changes[d].value).to_string();
            let new_prefix = leading_ws(&changes[i].value).to_string();
            let new_suffix = trailing_ws(&changes[i].value).to_string();
            if let Some(s) = start {
                let common = longest_common_prefix(&old_prefix, &new_prefix).to_string();
                changes[s].value = replace_suffix(&changes[s].value, &new_prefix, &common);
                changes[d].value = replace_prefix(&changes[d].value, &common, "");
                changes[i].value = replace_prefix(&changes[i].value, &common, "");
            }
            if let Some(e) = end {
                let common = longest_common_suffix(&old_suffix, &new_suffix).to_string();
                changes[e].value = replace_prefix(&changes[e].value, &new_suffix, &common);
                changes[d].value = replace_suffix(&changes[d].value, &common, "");
                changes[i].value = replace_suffix(&changes[i].value, &common, "");
            }
        }
        (None, Some(i)) => {
            if start.is_some() {
                let ws = leading_ws(&changes[i].value).len();
                changes[i].value = changes[i].value[ws..].to_string();
            }
            if let Some(e) = end {
                let ws = leading_ws(&changes[e].value).len();
                changes[e].value = changes[e].value[ws..].to_string();
            }
        }
        (Some(d), None) => match (start, end) {
            (Some(s), Some(e)) => {
                let new_ws_full = leading_ws(&changes[e].value).to_string();
                let del_ws_start = leading_ws(&changes[d].value).to_string();
                let del_ws_end = trailing_ws(&changes[d].value).to_string();
                let new_ws_start = longest_common_prefix(&new_ws_full, &del_ws_start).to_string();
                changes[d].value = replace_prefix(&changes[d].value, &new_ws_start, "");
                let rest = replace_prefix(&new_ws_full, &new_ws_start, "");
                let new_ws_end = longest_common_suffix(&rest, &del_ws_end).to_string();
                changes[d].value = replace_suffix(&changes[d].value, &new_ws_end, "");
                changes[e].value = replace_prefix(&changes[e].value, &new_ws_full, &new_ws_end);
                let keep = &new_ws_full[..new_ws_full.len() - new_ws_end.len()];
                changes[s].value = replace_suffix(&changes[s].value, &new_ws_full, keep);
            }
            (None, Some(e)) => {
                let end_prefix = leading_ws(&changes[e].value).to_string();
                let del_suffix = trailing_ws(&changes[d].value).to_string();
                let overlap = maximum_overlap(&del_suffix, &end_prefix).to_string();
                changes[d].value = replace_suffix(&changes[d].value, &overlap, "");
            }
            (Some(s), None) => {
                let start_suffix = trailing_ws(&changes[s].value).to_string();
                let del_prefix = leading_ws(&changes[d].value).to_string();
                let overlap = maximum_overlap(&start_suffix, &del_prefix).to_string();
                changes[d].value = replace_prefix(&changes[d].value, &overlap, "");
            }
            (None, None) => {}
        },
        (None, None) => {}
    }
}

/// `WordDiff.postProcess`.
fn post_process(mut changes: Vec<Change>) -> Vec<Change> {
    let mut last_keep: Option<usize> = None;
    let mut insertion: Option<usize> = None;
    let mut deletion: Option<usize> = None;
    for i in 0..changes.len() {
        if changes[i].added {
            insertion = Some(i);
        } else if changes[i].removed {
            deletion = Some(i);
        } else {
            if insertion.is_some() || deletion.is_some() {
                dedupe_whitespace(&mut changes, last_keep, deletion, insertion, Some(i));
            }
            last_keep = Some(i);
            insertion = None;
            deletion = None;
        }
    }
    if insertion.is_some() || deletion.is_some() {
        dedupe_whitespace(&mut changes, last_keep, deletion, insertion, None);
    }
    changes
}

/// `Diff.diffWords(oldStr, newStr)`.
pub fn diff_words(old: &str, new: &str) -> Vec<Change> {
    let old_tokens: Vec<String> = tokenize(old)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    let new_tokens: Vec<String> = tokenize(new)
        .into_iter()
        .filter(|t| !t.is_empty())
        .collect();
    post_process(diff_tokens(&old_tokens, &new_tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(changes: &[Change]) -> String {
        changes
            .iter()
            .map(|c| {
                let tag = if c.added {
                    "+"
                } else if c.removed {
                    "-"
                } else {
                    "="
                };
                format!("{tag}{:?}", c.value)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn documented_whitespace_cases() {
        assert_eq!(
            show(&diff_words("foo bar baz", "foo baz")),
            r#"="foo " -"bar " ="baz""#
        );
        assert_eq!(
            show(&diff_words("foo bar baz", "foo qux baz")),
            r#"="foo " -"bar" +"qux" =" baz""#
        );
        assert_eq!(
            show(&diff_words("foo   bar baz", "foo  baz")),
            r#"="foo  " -" bar " ="baz""#
        );
    }
}
