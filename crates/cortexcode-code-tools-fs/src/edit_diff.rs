//! `core/tools/edit-diff.ts`: matching `oldText` against a file (exact, then
//! fuzzy-normalized, then indentation-tolerant line blocks), applying edits,
//! and the line-numbered diff shown to the user.
//!
//! Offsets are UTF-16 code units, as in hoocode: match spans, the error
//! messages' columns and the sort order of edits all follow JS string
//! indices.

use std::path::Path;
use std::sync::LazyLock;

use unicode_normalization::UnicodeNormalization;

/// One replacement (`Edit`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Edit {
    pub old_text: String,
    pub new_text: String,
    /// Replace every occurrence instead of requiring a unique match.
    pub replace_all: bool,
}

impl Edit {
    pub fn new(old_text: impl Into<String>, new_text: impl Into<String>) -> Self {
        Self {
            old_text: old_text.into(),
            new_text: new_text.into(),
            replace_all: false,
        }
    }
}

/// `AppliedEditsResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdits {
    pub base_content: String,
    pub new_content: String,
}

/// `EditDiffResult`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditDiff {
    pub diff: String,
    pub first_changed_line: Option<usize>,
}

// ---------------------------------------------------------------------------
// UTF-16 helpers
// ---------------------------------------------------------------------------

type U16 = Vec<u16>;

fn utf16(s: &str) -> U16 {
    s.encode_utf16().collect()
}

fn string(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

const LF: u16 = b'\n' as u16;
const CR: u16 = b'\r' as u16;
const TAB: u16 = b'\t' as u16;
const SPACE: u16 = b' ' as u16;

/// JS `\s`.
fn is_js_space(u: u16) -> bool {
    matches!(
        u,
        0x09..=0x0d
            | 0x20
            | 0xa0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

/// `haystack.indexOf(needle, from)`.
fn index_of(haystack: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return (from <= haystack.len()).then_some(from);
    }
    if needle.len() > haystack.len() {
        return None;
    }
    let first = needle[0];
    (from..=haystack.len() - needle.len())
        .find(|&i| haystack[i] == first && haystack[i..i + needle.len()] == *needle)
}

fn split_lines(text: &[u16]) -> Vec<&[u16]> {
    text.split(|&u| u == LF).collect()
}

fn join_lines(lines: &[U16]) -> U16 {
    let mut out = U16::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push(LF);
        }
        out.extend_from_slice(line);
    }
    out
}

/// Length of the leading `[ \t]*`.
fn indent_len(line: &[u16]) -> usize {
    line.iter().take_while(|&&u| u == SPACE || u == TAB).count()
}

/// JS `trim()`.
fn js_trim(text: &[u16]) -> &[u16] {
    let start = text.iter().take_while(|&&u| is_js_space(u)).count();
    let end = text.len()
        - text[start..]
            .iter()
            .rev()
            .take_while(|&&u| is_js_space(u))
            .count();
    &text[start..end]
}

/// The code point at `i` and its UTF-16 length (a lone surrogate is itself).
fn code_point_at(text: &[u16], i: usize) -> (u32, usize) {
    let unit = text[i];
    if (0xd800..0xdc00).contains(&unit) {
        if let Some(&low) = text.get(i + 1) {
            if (0xdc00..0xe000).contains(&low) {
                let cp = 0x10000 + ((u32::from(unit) - 0xd800) << 10) + (u32::from(low) - 0xdc00);
                return (cp, 2);
            }
        }
    }
    (u32::from(unit), 1)
}

/// `str.normalize("NFKC")` on UTF-16 (lone surrogates pass through).
fn nfkc(text: &[u16]) -> U16 {
    match String::from_utf16(text) {
        Ok(s) => s.nfkc().collect::<String>().encode_utf16().collect(),
        Err(_) => text.to_vec(),
    }
}

static COMBINING_MARK: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\p{Mn}$").expect("combining mark pattern"));

fn is_combining_mark(cp: u32) -> bool {
    char::from_u32(cp).is_some_and(|c| {
        let mut buf = [0u8; 4];
        COMBINING_MARK.is_match(c.encode_utf8(&mut buf))
    })
}

// ---------------------------------------------------------------------------
// Line endings and BOM
// ---------------------------------------------------------------------------

/// `detectLineEnding`.
pub fn detect_line_ending(content: &str) -> &'static str {
    match (content.find("\r\n"), content.find('\n')) {
        (Some(crlf), Some(lf)) if crlf < lf => "\r\n",
        _ => "\n",
    }
}

/// `normalizeToLF`.
pub fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `restoreLineEndings`.
pub fn restore_line_endings(text: &str, ending: &str) -> String {
    if ending == "\r\n" {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

/// `stripBom`: `(bom, text)`.
pub fn strip_bom(content: &str) -> (&'static str, &str) {
    match content.strip_prefix('\u{feff}') {
        Some(text) => ("\u{feff}", text),
        None => ("", content),
    }
}

// ---------------------------------------------------------------------------
// Fuzzy normalization
// ---------------------------------------------------------------------------

/// A normalized text plus, per unit, the source index that produced it (and
/// one trailing entry for the end).
struct NormalizedWithMap {
    text: U16,
    map: Vec<usize>,
}

/// NFKC per grapheme-ish cluster (a base plus following Mn marks).
fn nfkc_with_map(text: &[u16]) -> NormalizedWithMap {
    let mut out = U16::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let start = i;
        let (_, len) = code_point_at(text, i);
        i += len;
        while i < text.len() {
            let (cp, len) = code_point_at(text, i);
            if !is_combining_mark(cp) {
                break;
            }
            i += len;
        }
        let composed = nfkc(&text[start..i]);
        map.extend(std::iter::repeat_n(start, composed.len()));
        out.extend(composed);
    }
    map.push(text.len());
    NormalizedWithMap { text: out, map }
}

/// CRLF and lone CR collapse to LF.
fn to_lf_with_map(text: &[u16]) -> NormalizedWithMap {
    let mut out = U16::new();
    let mut map = Vec::new();
    let mut i = 0;
    while i < text.len() {
        map.push(i);
        if text[i] == CR {
            out.push(LF);
            if text.get(i + 1) == Some(&LF) {
                i += 1;
            }
        } else {
            out.push(text[i]);
        }
        i += 1;
    }
    map.push(text.len());
    NormalizedWithMap { text: out, map }
}

/// Tabs widen to two spaces, interior runs of spaces collapse (indentation
/// kept), trailing whitespace dropped; per line.
fn normalize_line_whitespace_with_map(text: &[u16]) -> NormalizedWithMap {
    let mut out = U16::new();
    let mut map = Vec::new();
    let mut line_start = 0;
    loop {
        let (line_end, has_newline) = match index_of(text, &[LF], line_start) {
            Some(end) => (end, true),
            None => (text.len(), false),
        };
        let mut expanded = U16::new();
        let mut expanded_map = Vec::new();
        for (i, &unit) in text.iter().enumerate().take(line_end).skip(line_start) {
            if unit == TAB {
                expanded.extend([SPACE, SPACE]);
                expanded_map.extend([i, i]);
            } else {
                expanded.push(unit);
                expanded_map.push(i);
            }
        }
        let leading = expanded.iter().take_while(|&&u| is_js_space(u)).count();
        let mut emitted = U16::new();
        let mut emitted_map = Vec::new();
        for (i, &unit) in expanded.iter().enumerate() {
            if i >= leading
                && unit == SPACE
                && emitted.last() == Some(&SPACE)
                && emitted.len() > leading
            {
                continue;
            }
            emitted.push(unit);
            emitted_map.push(expanded_map[i]);
        }
        let mut end = emitted.len();
        while end > 0 && is_js_space(emitted[end - 1]) {
            end -= 1;
        }
        out.extend_from_slice(&emitted[..end]);
        map.extend_from_slice(&emitted_map[..end]);
        if has_newline {
            out.push(LF);
            map.push(line_end);
            line_start = line_end + 1;
        } else {
            break;
        }
    }
    map.push(text.len());
    NormalizedWithMap { text: out, map }
}

fn compose_maps(outer: &[usize], inner: &[usize]) -> Vec<usize> {
    let last = inner.last().copied().unwrap_or(0);
    outer
        .iter()
        .map(|&i| inner.get(i).copied().unwrap_or(last))
        .collect()
}

/// Quotes, dashes and exotic spaces, one-for-one.
fn substitute_char(unit: u16) -> u16 {
    match unit {
        0x2018..=0x201b => b'\'' as u16,
        0x201c..=0x201f => b'"' as u16,
        0x2010..=0x2015 | 0x2212 => b'-' as u16,
        0x00a0 | 0x2002..=0x200a | 0x202f | 0x205f | 0x3000 => SPACE,
        other => other,
    }
}

fn apply_char_substitutions(text: &mut [u16]) {
    for unit in text {
        *unit = substitute_char(*unit);
    }
}

fn normalize_for_fuzzy_match_with_map(text: &[u16]) -> NormalizedWithMap {
    let nfkc = nfkc_with_map(text);
    let lf = to_lf_with_map(&nfkc.text);
    let mut lines = normalize_line_whitespace_with_map(&lf.text);
    apply_char_substitutions(&mut lines.text);
    let map = compose_maps(&compose_maps(&lines.map, &lf.map), &nfkc.map);
    NormalizedWithMap {
        text: lines.text,
        map,
    }
}

/// `normalizeForFuzzyMatch`.
fn normalize_for_fuzzy_match(text: &[u16]) -> U16 {
    let nfkc = nfkc(text);
    let mut lf = U16::with_capacity(nfkc.len());
    let mut i = 0;
    while i < nfkc.len() {
        if nfkc[i] == CR {
            lf.push(LF);
            if nfkc.get(i + 1) == Some(&LF) {
                i += 1;
            }
        } else {
            lf.push(nfkc[i]);
        }
        i += 1;
    }
    let lines: Vec<U16> = split_lines(&lf)
        .into_iter()
        .map(|line| {
            let mut expanded = U16::with_capacity(line.len());
            for &u in line {
                if u == TAB {
                    expanded.extend([SPACE, SPACE]);
                } else {
                    expanded.push(u);
                }
            }
            let leading = expanded.iter().take_while(|&&u| is_js_space(u)).count();
            let mut normalized = expanded[..leading].to_vec();
            // rest.replace(/ {2,}/g, " ")
            for &u in &expanded[leading..] {
                if u == SPACE && normalized.len() > leading && normalized.last() == Some(&SPACE) {
                    continue;
                }
                normalized.push(u);
            }
            let end = normalized.len()
                - normalized
                    .iter()
                    .rev()
                    .take_while(|&&u| is_js_space(u))
                    .count();
            normalized.truncate(end);
            normalized
        })
        .collect();
    let mut joined = join_lines(&lines);
    apply_char_substitutions(&mut joined);
    joined
}

fn is_at_line_start(text: &[u16], index: usize) -> bool {
    index == 0 || text[index - 1] == LF
}

/// Leading `[ \t]*` with tabs widened to two spaces.
fn normalized_indent(line: &[u16]) -> U16 {
    let mut out = U16::new();
    for &u in &line[..indent_len(line)] {
        if u == TAB {
            out.extend([SPACE, SPACE]);
        } else {
            out.push(u);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ResolvedSpan {
    match_index: usize,
    match_length: usize,
    replacement: U16,
}

struct MatchedEdit {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    replacement: U16,
}

struct EditU16 {
    old_text: U16,
    new_text: U16,
    replace_all: bool,
}

struct FoundSpans {
    spans: Vec<ResolvedSpan>,
    occurrences: usize,
    noop_fuzzy_span: Option<ResolvedSpan>,
}

/// Every non-overlapping occurrence of `needle`.
fn collect_match_indices(haystack: &[u16], needle: &[u16]) -> Vec<usize> {
    let mut indices = Vec::new();
    if needle.is_empty() {
        return indices;
    }
    let mut from = 0;
    while let Some(i) = index_of(haystack, needle, from) {
        indices.push(i);
        from = i + needle.len();
    }
    indices
}

fn find_edit_spans(
    content: &[u16],
    edit: &EditU16,
    fuzzy_index: &mut dyn FnMut() -> Option<std::rc::Rc<NormalizedWithMap>>,
) -> FoundSpans {
    // An oldText that opens with indentation claims a whole line.
    let anchored = matches!(edit.old_text.first(), Some(&SPACE) | Some(&TAB));

    // Tier 1: exact.
    let exact: Vec<usize> = collect_match_indices(content, &edit.old_text)
        .into_iter()
        .filter(|&i| !anchored || is_at_line_start(content, i))
        .collect();
    if !exact.is_empty() {
        return FoundSpans {
            occurrences: exact.len(),
            spans: exact
                .into_iter()
                .map(|match_index| ResolvedSpan {
                    match_index,
                    match_length: edit.old_text.len(),
                    replacement: edit.new_text.clone(),
                })
                .collect(),
            noop_fuzzy_span: None,
        };
    }

    // Tier 2: fuzzy, mapped back onto the source bytes it matched.
    if let Some(normalized) = fuzzy_index() {
        let fuzzy_old = normalize_for_fuzzy_match(&edit.old_text);
        let fuzzy: Vec<usize> = collect_match_indices(&normalized.text, &fuzzy_old)
            .into_iter()
            .filter(|&i| !anchored || is_at_line_start(&normalized.text, i))
            .collect();
        let bounds = |index: usize| {
            let start = normalized.map[index];
            let end = normalized.map[index + fuzzy_old.len()];
            (start, end)
        };
        // newText keeps the file's own indentation where it says the same
        // thing as oldText's (a fuzzy match means oldText's whitespace was the
        // model's rendering, not the file's).
        let replacement_for = |index: usize| -> U16 {
            let (start, end) = bounds(index);
            if !is_at_line_start(content, start) {
                return edit.new_text.clone();
            }
            let original_lines = split_lines(&content[start..end]);
            let new_lines = split_lines(&edit.new_text);
            let old_lines = split_lines(&edit.old_text);
            if original_lines.len() != new_lines.len() || old_lines.len() != new_lines.len() {
                return edit.new_text.clone();
            }
            let lines: Vec<U16> = new_lines
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let new_indent = &line[..indent_len(line)];
                    let old_indent = &old_lines[i][..indent_len(old_lines[i])];
                    if normalized_indent(new_indent) != normalized_indent(old_indent) {
                        return line.to_vec();
                    }
                    let mut out = original_lines[i][..indent_len(original_lines[i])].to_vec();
                    out.extend_from_slice(&line[new_indent.len()..]);
                    out
                })
                .collect();
            join_lines(&lines)
        };

        if !fuzzy.is_empty() {
            if normalize_for_fuzzy_match(&edit.new_text) == fuzzy_old {
                // newText asks for nothing the matcher can see: leave the bytes
                // alone and report the span.
                let untouched: Vec<ResolvedSpan> = fuzzy
                    .iter()
                    .map(|&index| {
                        let (start, end) = bounds(index);
                        ResolvedSpan {
                            match_index: start,
                            match_length: end - start,
                            replacement: content[start..end].to_vec(),
                        }
                    })
                    .collect();
                return FoundSpans {
                    occurrences: fuzzy.len(),
                    noop_fuzzy_span: untouched.first().cloned(),
                    spans: untouched,
                };
            }
            return FoundSpans {
                occurrences: fuzzy.len(),
                spans: fuzzy
                    .iter()
                    .map(|&index| {
                        let (start, end) = bounds(index);
                        ResolvedSpan {
                            match_index: start,
                            match_length: end - start,
                            replacement: replacement_for(index),
                        }
                    })
                    .collect(),
                noop_fuzzy_span: None,
            };
        }
    }

    // Tier 3: indentation-tolerant line blocks.
    let blocks: Vec<ResolvedSpan> = find_line_block_matches(content, &edit.old_text, anchored)
        .into_iter()
        .map(|(match_index, match_length)| ResolvedSpan {
            match_index,
            match_length,
            replacement: edit.new_text.clone(),
        })
        .collect();
    FoundSpans {
        occurrences: blocks.len(),
        spans: blocks,
        noop_fuzzy_span: None,
    }
}

fn block_normalize_line(line: &[u16]) -> U16 {
    js_trim(&normalize_for_fuzzy_match(line)).to_vec()
}

/// Blocks whose lines equal oldText's lines ignoring leading/trailing
/// whitespace; `(matchIndex, matchLength)` in `content`.
fn find_line_block_matches(
    content: &[u16],
    old_text: &[u16],
    anchored: bool,
) -> Vec<(usize, usize)> {
    let had_trailing_newline = old_text.last() == Some(&LF);
    let mut old_lines = split_lines(old_text);
    if had_trailing_newline {
        old_lines.pop();
    }
    if old_lines.is_empty() {
        return Vec::new();
    }
    let trimmed_old: Vec<U16> = old_lines.iter().map(|l| block_normalize_line(l)).collect();
    let content_lines = split_lines(content);
    let k = trimmed_old.len();
    if k > content_lines.len() {
        return Vec::new();
    }
    let mut offsets = Vec::with_capacity(content_lines.len());
    let mut acc = 0;
    for line in &content_lines {
        offsets.push(acc);
        acc += line.len() + 1;
    }
    let normalized_content: Vec<Option<U16>> = vec![None; content_lines.len()];
    let mut normalized_content = normalized_content;
    let mut matches = Vec::new();
    for i in 0..=content_lines.len() - k {
        let mut ok = true;
        for j in 0..k {
            let normalized = normalized_content[i + j]
                .get_or_insert_with(|| block_normalize_line(content_lines[i + j]));
            if *normalized != trimmed_old[j] {
                ok = false;
                break;
            }
            if anchored
                && normalized_indent(content_lines[i + j]) != normalized_indent(old_lines[j])
            {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        let mut match_length = 0;
        for j in 0..k {
            match_length += content_lines[i + j].len() + usize::from(j < k - 1);
        }
        if had_trailing_newline && i + k < content_lines.len() {
            match_length += 1;
        }
        matches.push((offsets[i], match_length));
    }
    matches
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn not_found_error(path: &str, edit_index: usize, total: usize) -> String {
    if total == 1 {
        format!("Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.")
    } else {
        format!("Could not find edits[{edit_index}] in {path}. The oldText must match exactly including all whitespace and newlines.")
    }
}

fn duplicate_error(path: &str, edit_index: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!("Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.")
    } else {
        format!("Found {occurrences} occurrences of edits[{edit_index}] in {path}. Each oldText must be unique. Please provide more context to make it unique.")
    }
}

fn empty_old_text_error(path: &str, edit_index: usize, total: usize) -> String {
    if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{edit_index}].oldText must not be empty in {path}.")
    }
}

/// Characters fuzzy normalization erases, by name.
fn normalized_away_name(cp: u32) -> Option<&'static str> {
    Some(match cp {
        0x0009 => "TAB",
        0x00a0 => "NO-BREAK SPACE",
        0x2002 => "EN SPACE",
        0x2003 => "EM SPACE",
        0x2009 => "THIN SPACE",
        0x200a => "HAIR SPACE",
        0x2010 => "HYPHEN",
        0x2011 => "NON-BREAKING HYPHEN",
        0x2012 => "FIGURE DASH",
        0x2013 => "EN DASH",
        0x2014 => "EM DASH",
        0x2015 => "HORIZONTAL BAR",
        0x2018 => "LEFT SINGLE QUOTATION MARK",
        0x2019 => "RIGHT SINGLE QUOTATION MARK",
        0x201a => "SINGLE LOW-9 QUOTATION MARK",
        0x201b => "SINGLE HIGH-REVERSED-9 QUOTATION MARK",
        0x201c => "LEFT DOUBLE QUOTATION MARK",
        0x201d => "RIGHT DOUBLE QUOTATION MARK",
        0x201e => "DOUBLE LOW-9 QUOTATION MARK",
        0x201f => "DOUBLE HIGH-REVERSED-9 QUOTATION MARK",
        0x202f => "NARROW NO-BREAK SPACE",
        0x205f => "MEDIUM MATHEMATICAL SPACE",
        0x2212 => "MINUS SIGN",
        0x3000 => "IDEOGRAPHIC SPACE",
        _ => return None,
    })
}

fn format_code_point(ch: &[u16]) -> String {
    let (cp, _) = code_point_at(ch, 0);
    let hex = format!("U+{cp:04X}");
    match normalized_away_name(cp) {
        Some(name) => format!("{hex} {name}"),
        None => {
            let normalized = string(&nfkc(ch));
            format!(
                "{hex} (normalizes to {})",
                serde_json::to_string(&normalized).expect("string serializes")
            )
        }
    }
}

const MAX_REPORTED_CHARS: usize = 5;

/// Every character normalization would change, with its UTF-16 index.
fn find_normalized_away_chars(text: &[u16]) -> Vec<(U16, usize)> {
    let mut found = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let (cp, len) = code_point_at(text, index);
        let ch = &text[index..index + len];
        if normalized_away_name(cp).is_some() || nfkc(ch) != ch {
            found.push((ch.to_vec(), index));
        }
        index += len;
    }
    found
}

/// 1-indexed line and column (UTF-16) of `offset`.
fn line_and_column(content: &[u16], offset: usize) -> (usize, usize) {
    let before = &content[..offset];
    let line = before.iter().filter(|&&u| u == LF).count() + 1;
    let line_start = before.iter().rposition(|&u| u == LF).map_or(0, |i| i + 1);
    (line, offset - line_start + 1)
}

fn fuzzy_noop_error(
    path: &str,
    edit_index: usize,
    total: usize,
    content: &[u16],
    span: &ResolvedSpan,
) -> String {
    let matched = &content[span.match_index..span.match_index + span.match_length];
    let which = if total == 1 {
        "The edit".to_owned()
    } else {
        format!("edits[{edit_index}]")
    };
    let offenders = find_normalized_away_chars(matched);
    let detail = if offenders.is_empty() {
        format!("oldText matched {path} only after whitespace normalization, and newText normalizes to the same text, so nothing would change. Send oldText exactly as the file spells it.")
    } else {
        let shown: Vec<String> = offenders
            .iter()
            .take(MAX_REPORTED_CHARS)
            .map(|(ch, index)| {
                let (line, column) = line_and_column(content, span.match_index + index);
                format!("{} at line {line}, column {column}", format_code_point(ch))
            })
            .collect();
        let more = offenders.len() - shown.len();
        let more = if more > 0 {
            format!("; and {more} more")
        } else {
            String::new()
        };
        format!(
            "the text it matched in {path} contains {}{more}, which your oldText spelled as plain-ASCII lookalikes. Send oldText containing those exact characters and the replacement will apply.",
            shown.join("; ")
        )
    };
    format!("No changes made to {path}. {which} asked for a change that is invisible to matching: {detail}")
}

fn no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!("No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.")
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

// ---------------------------------------------------------------------------
// Applying
// ---------------------------------------------------------------------------

/// `applyEditsToNormalizedContent`: match every edit against the same
/// original, reject empty/missing/duplicate/overlapping/no-op edits, and
/// apply them back to front. The error is the message the model sees.
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[Edit],
    path: &str,
) -> Result<AppliedEdits, String> {
    let normalized_edits: Vec<EditU16> = edits
        .iter()
        .map(|e| EditU16 {
            old_text: utf16(&normalize_to_lf(&e.old_text)),
            new_text: utf16(&normalize_to_lf(&e.new_text)),
            replace_all: e.replace_all,
        })
        .collect();
    let total = normalized_edits.len();
    if let Some(i) = normalized_edits.iter().position(|e| e.old_text.is_empty()) {
        return Err(empty_old_text_error(path, i, total));
    }

    let base = utf16(normalized_content);
    let mut cache: Option<Option<std::rc::Rc<NormalizedWithMap>>> = None;
    let mut fuzzy_index = || {
        cache
            .get_or_insert_with(|| {
                let built = normalize_for_fuzzy_match_with_map(&base);
                (built.map.len() == built.text.len() + 1).then(|| std::rc::Rc::new(built))
            })
            .clone()
    };

    let mut matched = Vec::new();
    for (i, edit) in normalized_edits.iter().enumerate() {
        let found = find_edit_spans(&base, edit, &mut fuzzy_index);
        if found.spans.is_empty() {
            return Err(not_found_error(path, i, total));
        }
        if let Some(span) = &found.noop_fuzzy_span {
            return Err(fuzzy_noop_error(path, i, total, &base, span));
        }
        if edit.replace_all {
            matched.extend(found.spans.into_iter().map(|span| MatchedEdit {
                edit_index: i,
                match_index: span.match_index,
                match_length: span.match_length,
                replacement: span.replacement,
            }));
            continue;
        }
        if found.occurrences > 1 {
            return Err(duplicate_error(path, i, total, found.occurrences));
        }
        let span = found.spans.into_iter().next().expect("non-empty");
        matched.push(MatchedEdit {
            edit_index: i,
            match_index: span.match_index,
            match_length: span.match_length,
            replacement: span.replacement,
        });
    }

    matched.sort_by_key(|m| m.match_index);
    for pair in matched.windows(2) {
        let (previous, current) = (&pair[0], &pair[1]);
        if previous.match_index + previous.match_length > current.match_index {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                previous.edit_index, current.edit_index
            ));
        }
    }

    let mut new_content = base.clone();
    for m in matched.iter().rev() {
        new_content.splice(
            m.match_index..m.match_index + m.match_length,
            m.replacement.iter().copied(),
        );
    }
    if base == new_content {
        return Err(no_change_error(path, total));
    }
    Ok(AppliedEdits {
        base_content: normalized_content.to_owned(),
        new_content: string(&new_content),
    })
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// A run of lines from jsdiff's `diffLines` (`added`/`removed`/common).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Common,
    Added,
    Removed,
}

/// jsdiff's line tokenizer: each line with its `\n` / `\r\n` attached.
fn tokenize_lines(value: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' {
            tokens.push(&value[start..=i]);
            start = i + 1;
        } else if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            tokens.push(&value[start..i + 2]);
            i += 1;
            start = i + 1;
        }
        i += 1;
    }
    if start < value.len() {
        tokens.push(&value[start..]);
    }
    tokens
}

/// One run in jsdiff's linked list of components.
struct Component {
    count: usize,
    added: bool,
    removed: bool,
    previous: Option<usize>,
}

#[derive(Clone, Copy)]
struct DiffPath {
    old_pos: isize,
    last: Option<usize>,
}

/// jsdiff 8 `Diff.diff` (Myers with its diagonal pruning and tie-breaking),
/// over line tokens, returning the runs in order.
fn diff_lines(old: &str, new: &str) -> Vec<(PartKind, String)> {
    let old_tokens = tokenize_lines(old);
    let new_tokens = tokenize_lines(new);
    let (old_len, new_len) = (old_tokens.len() as isize, new_tokens.len() as isize);
    let mut arena: Vec<Component> = Vec::new();

    let add_to_path = |arena: &mut Vec<Component>,
                       path: DiffPath,
                       added: bool,
                       removed: bool,
                       old_pos_inc: isize|
     -> DiffPath {
        let component = match path.last.map(|i| &arena[i]) {
            Some(last) if last.added == added && last.removed == removed => Component {
                count: last.count + 1,
                added,
                removed,
                previous: last.previous,
            },
            _ => Component {
                count: 1,
                added,
                removed,
                previous: path.last,
            },
        };
        arena.push(component);
        DiffPath {
            old_pos: path.old_pos + old_pos_inc,
            last: Some(arena.len() - 1),
        }
    };
    let extract_common =
        |arena: &mut Vec<Component>, path: &mut DiffPath, diagonal: isize| -> isize {
            let mut old_pos = path.old_pos;
            let mut new_pos = old_pos - diagonal;
            let mut common = 0;
            while new_pos + 1 < new_len
                && old_pos + 1 < old_len
                && old_tokens[(old_pos + 1) as usize] == new_tokens[(new_pos + 1) as usize]
            {
                new_pos += 1;
                old_pos += 1;
                common += 1;
            }
            if common > 0 {
                arena.push(Component {
                    count: common,
                    added: false,
                    removed: false,
                    previous: path.last,
                });
                path.last = Some(arena.len() - 1);
            }
            path.old_pos = old_pos;
            new_pos
        };
    let build = |arena: &Vec<Component>, last: Option<usize>| -> Vec<(PartKind, String)> {
        let mut chain = Vec::new();
        let mut next = last;
        while let Some(i) = next {
            chain.push(i);
            next = arena[i].previous;
        }
        chain.reverse();
        let (mut new_pos, mut old_pos) = (0, 0);
        chain
            .into_iter()
            .map(|i| {
                let c = &arena[i];
                if c.removed {
                    let value = old_tokens[old_pos..old_pos + c.count].concat();
                    old_pos += c.count;
                    (PartKind::Removed, value)
                } else {
                    let value = new_tokens[new_pos..new_pos + c.count].concat();
                    new_pos += c.count;
                    if c.added {
                        (PartKind::Added, value)
                    } else {
                        old_pos += c.count;
                        (PartKind::Common, value)
                    }
                }
            })
            .collect()
    };

    let max_edit = new_len + old_len;
    let offset = max_edit + 1;
    let mut best: Vec<Option<DiffPath>> = vec![None; (2 * max_edit + 3) as usize];
    let slot = |d: isize| (d + offset) as usize;

    let mut first = DiffPath {
        old_pos: -1,
        last: None,
    };
    let new_pos = extract_common(&mut arena, &mut first, 0);
    if first.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
        return build(&arena, first.last);
    }
    best[slot(0)] = Some(first);

    let mut min_diagonal = isize::MIN;
    let mut max_diagonal = isize::MAX;
    let mut edit_length: isize = 1;
    while edit_length <= max_edit {
        let mut diagonal = min_diagonal.max(-edit_length);
        while diagonal <= max_diagonal.min(edit_length) {
            let remove_path = best[slot(diagonal - 1)].take();
            let add_path = best[slot(diagonal + 1)];
            let can_add = add_path.is_some_and(|p| {
                let add_new_pos = p.old_pos - diagonal;
                0 <= add_new_pos && add_new_pos < new_len
            });
            let can_remove = remove_path.is_some_and(|p| p.old_pos + 1 < old_len);
            if !can_add && !can_remove {
                best[slot(diagonal)] = None;
                diagonal += 2;
                continue;
            }
            let mut base = if !can_remove
                || (can_add
                    && remove_path.expect("can_remove").old_pos
                        < add_path.expect("can_add").old_pos)
            {
                add_to_path(&mut arena, add_path.expect("can_add"), true, false, 0)
            } else {
                add_to_path(&mut arena, remove_path.expect("can_remove"), false, true, 1)
            };
            let new_pos = extract_common(&mut arena, &mut base, diagonal);
            if base.old_pos + 1 >= old_len && new_pos + 1 >= new_len {
                return build(&arena, base.last);
            }
            best[slot(diagonal)] = Some(base);
            if base.old_pos + 1 >= old_len {
                max_diagonal = max_diagonal.min(diagonal - 1);
            }
            if new_pos + 1 >= new_len {
                min_diagonal = min_diagonal.max(diagonal + 1);
            }
            diagonal += 2;
        }
        edit_length += 1;
    }
    unreachable!("a diff always exists within old + new edits")
}

/// `generateDiffString`: changed lines with line numbers and `context_lines`
/// of context, gaps shown as `...`.
pub fn generate_diff_string(
    old_content: &str,
    new_content: &str,
    context_lines: usize,
) -> EditDiff {
    let parts = diff_lines(old_content, new_content);
    let mut output: Vec<String> = Vec::new();
    let max_line_num = old_content
        .split('\n')
        .count()
        .max(new_content.split('\n').count());
    let width = max_line_num.to_string().len();
    let num = |n: usize| format!("{n:>width$}");
    let gap = format!(" {} ...", " ".repeat(width));

    let mut old_line = 1;
    let mut new_line = 1;
    let mut last_was_change = false;
    let mut first_changed_line = None;

    for (i, (kind, value)) in parts.iter().enumerate() {
        let mut raw: Vec<&str> = value.split('\n').collect();
        if raw.last() == Some(&"") {
            raw.pop();
        }
        match kind {
            PartKind::Added | PartKind::Removed => {
                first_changed_line.get_or_insert(new_line);
                for line in raw {
                    if *kind == PartKind::Added {
                        output.push(format!("+{} {line}", num(new_line)));
                        new_line += 1;
                    } else {
                        output.push(format!("-{} {line}", num(old_line)));
                        old_line += 1;
                    }
                }
                last_was_change = true;
            }
            PartKind::Common => {
                let next_is_change = parts
                    .get(i + 1)
                    .is_some_and(|(k, _)| *k != PartKind::Common);
                let context = |output: &mut Vec<String>,
                               line: &str,
                               old_line: &mut usize,
                               new_line: &mut usize| {
                    output.push(format!(" {} {line}", num(*old_line)));
                    *old_line += 1;
                    *new_line += 1;
                };
                if last_was_change && next_is_change {
                    if raw.len() <= context_lines * 2 {
                        for line in &raw {
                            context(&mut output, line, &mut old_line, &mut new_line);
                        }
                    } else {
                        let skipped = raw.len() - 2 * context_lines;
                        for line in &raw[..context_lines] {
                            context(&mut output, line, &mut old_line, &mut new_line);
                        }
                        output.push(gap.clone());
                        old_line += skipped;
                        new_line += skipped;
                        for line in &raw[raw.len() - context_lines..] {
                            context(&mut output, line, &mut old_line, &mut new_line);
                        }
                    }
                } else if last_was_change {
                    let shown = raw.len().min(context_lines);
                    for line in &raw[..shown] {
                        context(&mut output, line, &mut old_line, &mut new_line);
                    }
                    let skipped = raw.len() - shown;
                    if skipped > 0 {
                        output.push(gap.clone());
                        old_line += skipped;
                        new_line += skipped;
                    }
                } else if next_is_change {
                    let skipped = raw.len().saturating_sub(context_lines);
                    if skipped > 0 {
                        output.push(gap.clone());
                        old_line += skipped;
                        new_line += skipped;
                    }
                    for line in &raw[skipped..] {
                        context(&mut output, line, &mut old_line, &mut new_line);
                    }
                } else {
                    old_line += raw.len();
                    new_line += raw.len();
                }
                last_was_change = false;
            }
        }
    }
    EditDiff {
        diff: output.join("\n"),
        first_changed_line,
    }
}

/// `computeEditsDiff`: the diff an edit would make, without applying it (the
/// TUI preview). Errors come back as the message.
pub fn compute_edits_diff(path: &str, edits: &[Edit], cwd: &Path) -> Result<EditDiff, String> {
    let absolute = cortexcode_code_tool_api::resolve_to_cwd(path, cwd);
    if let Err(e) = crate::access::access(&absolute, false) {
        let code = cortexcode_code_tool_api::node_fs_error(e, "access", None).code;
        return Err(format!("Could not edit file: {path}. Error code: {code}."));
    }
    let raw = std::fs::read(&absolute).map_err(|e| {
        cortexcode_code_tool_api::node_fs_error(e, "open", Some(&absolute.to_string_lossy()))
            .message
    })?;
    let raw = String::from_utf8_lossy(&raw);
    let (_, content) = strip_bom(&raw);
    let normalized = normalize_to_lf(content);
    let applied = apply_edits_to_normalized_content(&normalized, edits, path)?;
    Ok(generate_diff_string(
        &applied.base_content,
        &applied.new_content,
        4,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_map_matches_the_plain_normalization() {
        for text in [
            "a\t b  c   \r\nd\u{2019}e\u{00a0}f  \n  x  y",
            "e\u{301}\u{fb01}x\r\u{2014}",
            "",
        ] {
            let units = utf16(text);
            let with_map = normalize_for_fuzzy_match_with_map(&units);
            assert_eq!(with_map.text, normalize_for_fuzzy_match(&units), "{text:?}");
            assert_eq!(with_map.map.len(), with_map.text.len() + 1);
        }
    }

    #[test]
    fn line_endings_and_bom() {
        assert_eq!(detect_line_ending("a\r\nb\n"), "\r\n");
        assert_eq!(detect_line_ending("a\nb\r\n"), "\n");
        assert_eq!(detect_line_ending("abc"), "\n");
        assert_eq!(normalize_to_lf("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(restore_line_endings("a\nb", "\r\n"), "a\r\nb");
        assert_eq!(strip_bom("\u{feff}x"), ("\u{feff}", "x"));
    }

    #[test]
    fn diff_string_numbers_lines_and_elides_gaps() {
        let old: String = (1..=20).map(|i| format!("line {i}\n")).collect();
        let new = old
            .replace("line 3\n", "line three\n")
            .replace("line 18\n", "line eighteen\n");
        let diff = generate_diff_string(&old, &new, 4);
        assert_eq!(diff.first_changed_line, Some(3));
        assert_eq!(
            diff.diff,
            [
                "  1 line 1",
                "  2 line 2",
                "- 3 line 3",
                "+ 3 line three",
                "  4 line 4",
                "  5 line 5",
                "  6 line 6",
                "  7 line 7",
                "    ...",
                " 14 line 14",
                " 15 line 15",
                " 16 line 16",
                " 17 line 17",
                "-18 line 18",
                "+18 line eighteen",
                " 19 line 19",
                " 20 line 20",
            ]
            .join("\n")
        );
    }
}
