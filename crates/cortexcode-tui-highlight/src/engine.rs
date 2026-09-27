//! highlight.js 10.7.3's `core.js`: the mode compiler (`compileLanguage`),
//! the scanners (`MultiRegex`, `ResumableMultiRegex`), the token tree and
//! `_highlight` / `highlightAuto`, ported step by step.
//!
//! Positions are byte offsets where the original uses UTF-16 indices; the
//! places that step "one character" step one `char`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cortexcode_tui_util::js_regex::JsRegex;

use crate::value::{
    delete, get, inherit, new_arr, new_obj, set, str_v, Callback, CompiledRe, Obj, V,
};

// ---- regex helpers ---------------------------------------------------------

/// `source(re)`.
fn source(v: &V) -> Option<String> {
    match v {
        V::Str(s) => Some(s.to_string()),
        V::Re(r) => Some(r.source.clone()),
        V::Compiled(c) => Some(c.source.clone()),
        _ => None,
    }
}

/// `escape(value)`: a literal regex (flags `m`).
fn escape_re(value: &str) -> Rc<CompiledRe> {
    let mut out = String::new();
    for c in value.chars() {
        if "-/\\^$*+?.()|[]{}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    compile_re(&out, "m")
}

thread_local! {
    static RE_CACHE: RefCell<HashMap<(String, String), Rc<CompiledRe>>> = RefCell::new(HashMap::new());
}

fn compile_re(source: &str, flags: &str) -> Rc<CompiledRe> {
    let key = (source.to_string(), flags.to_string());
    if let Some(re) = RE_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return re;
    }
    // A source the translation cannot express never matches (logged once).
    let re = JsRegex::try_new(source, flags).unwrap_or_else(|_| JsRegex::new(r"[^\s\S]", ""));
    let compiled = Rc::new(CompiledRe {
        source: source.to_string(),
        re,
    });
    RE_CACHE.with(|c| c.borrow_mut().insert(key, compiled.clone()));
    compiled
}

/// `countMatchGroups(re)`.
fn count_match_groups(source: &str) -> usize {
    compile_re(source, "").re.captures_len()
}

/// `join(regexps, "|")`: each in its own group, backreferences renumbered.
fn join(regexps: &[String]) -> String {
    let mut num_captures = 0usize;
    let mut out_parts = Vec::new();
    for regex in regexps {
        num_captures += 1;
        let offset = num_captures;
        let chars: Vec<char> = regex.chars().collect();
        let mut out = String::new();
        let mut i = 0;
        // BACKREF_RE = /\[(?:[^\\\]]|\\.)*\]|\(\??|\\([1-9][0-9]*)|\\./
        while i < chars.len() {
            match chars[i] {
                '[' => {
                    // `\[(?:[^\\\]]|\\.)*\]`; without a closing `]` the
                    // pattern fails here and the scan moves on.
                    let mut j = i + 1;
                    let mut closed = false;
                    while j < chars.len() {
                        match chars[j] {
                            '\\' if j + 1 < chars.len() => j += 2,
                            '\\' => break,
                            ']' => {
                                closed = true;
                                break;
                            }
                            _ => j += 1,
                        }
                    }
                    if closed {
                        out.extend(&chars[i..=j]);
                        i = j + 1;
                    } else {
                        out.push('[');
                        i += 1;
                    }
                }
                '(' => {
                    if chars.get(i + 1) == Some(&'?') {
                        out.push_str("(?");
                        i += 2;
                    } else {
                        out.push('(');
                        num_captures += 1;
                        i += 1;
                    }
                }
                '\\' if i + 1 < chars.len() => {
                    let next = chars[i + 1];
                    if ('1'..='9').contains(&next) {
                        let mut j = i + 2;
                        while j < chars.len() && chars[j].is_ascii_digit() {
                            j += 1;
                        }
                        let n: usize = chars[i + 1..j]
                            .iter()
                            .collect::<String>()
                            .parse()
                            .unwrap_or(0);
                        out.push_str(&format!("\\{}", n + offset));
                        i = j;
                    } else {
                        out.push('\\');
                        out.push(next);
                        i += 2;
                    }
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        out_parts.push(format!("({out})"));
    }
    out_parts.join("|")
}

// ---- scanners --------------------------------------------------------------

/// What a scanner rule stands for.
#[derive(Clone, Debug)]
pub enum RuleType {
    Begin(Obj),
    End,
    Illegal,
}

#[derive(Clone, Debug)]
struct RuleOpts {
    kind: RuleType,
    position: usize,
}

/// A match with its rule: `match[0..]` from the rule's group on.
#[derive(Clone, Debug)]
pub struct ScanMatch {
    pub groups: Vec<Option<String>>,
    pub index: usize,
    pub kind: RuleType,
    pub position: usize,
}

impl ScanMatch {
    pub fn lexeme(&self) -> &str {
        self.groups[0].as_deref().unwrap_or("")
    }
}

/// `MultiRegex`.
#[derive(Debug)]
struct MultiRegex {
    match_indexes: HashMap<usize, RuleOpts>,
    regexes: Vec<String>,
    match_at: usize,
    position: usize,
    matcher: Option<Rc<CompiledRe>>,
}

impl MultiRegex {
    fn new() -> Self {
        Self {
            match_indexes: HashMap::new(),
            regexes: Vec::new(),
            match_at: 1,
            position: 0,
            matcher: None,
        }
    }

    fn add_rule(&mut self, re: &str, kind: RuleType) {
        let opts = RuleOpts {
            kind,
            position: self.position,
        };
        self.position += 1;
        self.match_indexes.insert(self.match_at, opts);
        self.regexes.push(re.to_string());
        self.match_at += count_match_groups(re) + 1;
    }

    fn compile(&mut self, flags: &str) {
        if self.regexes.is_empty() {
            return;
        }
        self.matcher = Some(compile_re(&join(&self.regexes), flags));
    }

    fn exec(&self, s: &str, last_index: usize) -> Option<ScanMatch> {
        let matcher = self.matcher.as_ref()?;
        if last_index > s.len() {
            return None;
        }
        let m = matcher.re.exec_at(s, last_index)?;
        let i = (1..m.len()).find(|&i| m.range(i).is_some())?;
        let opts = self.match_indexes.get(&i)?.clone();
        Some(ScanMatch {
            groups: (i..m.len()).map(|g| m.get(g).map(String::from)).collect(),
            index: m.index(),
            kind: opts.kind,
            position: opts.position,
        })
    }
}

/// `ResumableMultiRegex`.
#[derive(Debug)]
pub struct ResumableMultiRegex {
    rules: Vec<(String, RuleType)>,
    multi_regexes: HashMap<usize, MultiRegex>,
    count: usize,
    pub last_index: usize,
    pub regex_index: usize,
    flags: String,
}

impl ResumableMultiRegex {
    fn new(flags: &str) -> Self {
        Self {
            rules: Vec::new(),
            multi_regexes: HashMap::new(),
            count: 0,
            last_index: 0,
            regex_index: 0,
            flags: flags.to_string(),
        }
    }

    fn matcher(&mut self, index: usize) -> &MultiRegex {
        if !self.multi_regexes.contains_key(&index) {
            let mut m = MultiRegex::new();
            for (re, kind) in self.rules.iter().skip(index) {
                m.add_rule(re, kind.clone());
            }
            m.compile(&self.flags);
            self.multi_regexes.insert(index, m);
        }
        &self.multi_regexes[&index]
    }

    fn resuming_scan_at_same_position(&self) -> bool {
        self.regex_index != 0
    }

    pub fn consider_all(&mut self) {
        self.regex_index = 0;
    }

    fn add_rule(&mut self, re: String, kind: RuleType) {
        if matches!(kind, RuleType::Begin(_)) {
            self.count += 1;
        }
        self.rules.push((re, kind));
    }

    pub fn exec(&mut self, s: &str) -> Option<ScanMatch> {
        let last_index = self.last_index;
        let index = self.regex_index;
        let mut result = self.matcher(index).exec(s, last_index);
        if self.resuming_scan_at_same_position() {
            let at_same = result.as_ref().is_some_and(|r| r.index == last_index);
            if !at_same {
                // Step one character on, as JavaScript steps one code unit.
                let next = s[last_index.min(s.len())..]
                    .chars()
                    .next()
                    .map_or(last_index + 1, |c| last_index + c.len_utf8());
                result = self.matcher(0).exec(s, next);
            }
        }
        if let Some(r) = &result {
            self.regex_index += r.position + 1;
            if self.regex_index == self.count {
                self.consider_all();
            }
        }
        result
    }
}

// ---- compiler --------------------------------------------------------------

const COMMON_KEYWORDS: [&str; 11] = [
    "of", "and", "for", "in", "not", "or", "if", "then", "parent", "list", "value",
];

fn score_for_keyword(keyword: &str, provided: Option<&str>) -> f64 {
    if let Some(p) = provided.filter(|p| !p.is_empty()) {
        return p.trim().parse::<f64>().unwrap_or(f64::NAN);
    }
    if COMMON_KEYWORDS.contains(&keyword.to_lowercase().as_str()) {
        0.0
    } else {
        1.0
    }
}

/// `compileKeywords`.
fn compile_keywords(raw: &V, case_insensitive: bool, class_name: &str, out: &Obj) {
    let compile_list = |list: Vec<String>| {
        for keyword in list {
            let keyword = if case_insensitive {
                keyword.to_lowercase()
            } else {
                keyword
            };
            let mut pair = keyword.split('|');
            let name = pair.next().unwrap_or("").to_string();
            let score = score_for_keyword(&name, pair.next());
            set(out, &name, V::Keyword(Rc::from(class_name), score));
        }
    };
    match raw {
        V::Str(s) => compile_list(s.split(' ').map(String::from).collect()),
        V::Arr(a) => compile_list(
            a.borrow()
                .iter()
                .map(|v| match v {
                    V::Str(s) => s.to_string(),
                    V::Num(n) => format!("{n}"),
                    _ => String::new(),
                })
                .collect(),
        ),
        V::Obj(o) => {
            let entries = o.borrow().entries();
            for (key, value) in entries {
                compile_keywords(&value, case_insensitive, &key, out);
            }
        }
        _ => {}
    }
}

pub struct Compiler<'a> {
    pub language: Obj,
    pub case_insensitive: bool,
    pub extensions: &'a [Callback],
}

impl Compiler<'_> {
    fn lang_flags(&self, global: bool) -> String {
        format!(
            "m{}{}",
            if self.case_insensitive { "i" } else { "" },
            if global { "g" } else { "" }
        )
    }

    fn lang_re(&self, value: &V, global: bool) -> V {
        let src = source(value).unwrap_or_default();
        V::Compiled(compile_re(&src, &self.lang_flags(global)))
    }

    /// `beforeMatch` (R's compiler extension).
    fn ext_before_match(&self, mode: &Obj) {
        let before = get(mode, "beforeMatch");
        if !before.truthy() {
            return;
        }
        let original = inherit(mode, &[]);
        let keys = mode.borrow().keys();
        for key in keys {
            delete(mode, &key);
        }
        let begin = format!(
            "{}(?={})",
            source(&before).unwrap_or_default(),
            source(&get(&original, "begin")).unwrap_or_default()
        );
        set(mode, "begin", str_v(&begin));
        set(&original, "endsParent", V::Bool(true));
        let starts = new_obj();
        set(&starts, "relevance", V::Num(0.0));
        set(&starts, "contains", new_arr(vec![V::Obj(original.clone())]));
        set(mode, "starts", V::Obj(starts));
        set(mode, "relevance", V::Num(0.0));
        delete(&original, "beforeMatch");
    }

    pub fn compile_mode(&self, mode: &Obj, parent: Option<&Obj>) {
        if get(mode, "isCompiled").truthy() {
            return;
        }
        // compileMatch
        let m = get(mode, "match");
        if m.truthy() {
            set(mode, "begin", m);
            delete(mode, "match");
        }
        for ext in self.extensions {
            if *ext == Callback::ExtBeforeMatch {
                self.ext_before_match(mode);
            }
        }
        set(mode, "__beforeBegin", V::Null);
        // beginKeywords
        if parent.is_some() {
            if let V::Str(bk) = get(mode, "beginKeywords") {
                if !bk.is_empty() {
                    let begin = format!(
                        "\\b({})(?!\\.)(?=\\b|\\s)",
                        bk.split(' ').collect::<Vec<_>>().join("|")
                    );
                    set(mode, "begin", str_v(&begin));
                    set(
                        mode,
                        "__beforeBegin",
                        V::Fn(Callback::SkipIfHasPrecedingDot),
                    );
                    if !get(mode, "keywords").truthy() {
                        set(mode, "keywords", V::Str(bk.clone()));
                    }
                    delete(mode, "beginKeywords");
                    if get(mode, "relevance").is_undefined() {
                        set(mode, "relevance", V::Num(0.0));
                    }
                }
            }
        }
        // compileIllegal
        if let V::Arr(list) = get(mode, "illegal") {
            let alts: Vec<String> = list.borrow().iter().filter_map(source).collect();
            set(mode, "illegal", str_v(&format!("({})", alts.join("|"))));
        }
        // compileRelevance
        if get(mode, "relevance").is_undefined() {
            set(mode, "relevance", V::Num(1.0));
        }
        set(mode, "isCompiled", V::Bool(true));

        let mut keyword_pattern = V::Undefined;
        let keywords = get(mode, "keywords");
        if keywords.is_js_object() {
            if let V::Obj(k) = &keywords {
                keyword_pattern = get(k, "$pattern");
                delete(k, "$pattern");
            }
        }
        if keywords.truthy() {
            let compiled = new_obj();
            compile_keywords(&keywords, self.case_insensitive, "keyword", &compiled);
            set(mode, "keywords", V::Obj(compiled));
        }
        if !keyword_pattern.truthy() {
            keyword_pattern = get(mode, "lexemes");
        }
        if !keyword_pattern.truthy() {
            keyword_pattern = str_v("\\w+");
        }
        set(
            mode,
            "keywordPatternRe",
            self.lang_re(&keyword_pattern, true),
        );

        if let Some(parent) = parent {
            if !get(mode, "begin").truthy() {
                set(mode, "begin", str_v("\\B|\\b"));
            }
            set(mode, "beginRe", self.lang_re(&get(mode, "begin"), false));
            if get(mode, "endSameAsBegin").truthy() {
                set(mode, "end", get(mode, "begin"));
            }
            if !get(mode, "end").truthy() && !get(mode, "endsWithParent").truthy() {
                set(mode, "end", str_v("\\B|\\b"));
            }
            let end = get(mode, "end");
            if end.truthy() {
                set(mode, "endRe", self.lang_re(&end, false));
            }
            let mut terminator = source(&end).filter(|s| !s.is_empty()).unwrap_or_default();
            let parent_terminator = get(parent, "terminatorEnd");
            if get(mode, "endsWithParent").truthy() && parent_terminator.truthy() {
                if end.truthy() {
                    terminator.push('|');
                }
                terminator.push_str(parent_terminator.as_str().unwrap_or(""));
            }
            set(mode, "terminatorEnd", str_v(&terminator));
        }
        let illegal = get(mode, "illegal");
        if illegal.truthy() {
            set(mode, "illegalRe", self.lang_re(&illegal, false));
        }
        if !get(mode, "contains").truthy() {
            set(mode, "contains", new_arr(Vec::new()));
        }
        let contains = get(mode, "contains").as_arr().unwrap_or_default();
        let mut expanded = Vec::new();
        for c in contains {
            let target = match &c {
                V::Str(s) if &**s == "self" => mode.clone(),
                V::Obj(o) => o.clone(),
                _ => continue,
            };
            match expand_or_clone_mode(&target) {
                Expanded::Many(list) => expanded.extend(list),
                Expanded::One(o) => expanded.push(V::Obj(o)),
            }
        }
        set(mode, "contains", new_arr(expanded.clone()));
        for c in &expanded {
            if let V::Obj(o) = c {
                self.compile_mode(o, Some(mode));
            }
        }
        if let V::Obj(starts) = get(mode, "starts") {
            self.compile_mode(&starts, parent);
        }
        let matcher = self.build_mode_regex(mode);
        set(mode, "matcher", V::Matcher(Rc::new(RefCell::new(matcher))));
    }

    fn build_mode_regex(&self, mode: &Obj) -> ResumableMultiRegex {
        let mut mm = ResumableMultiRegex::new(&self.lang_flags(true));
        for term in get(mode, "contains").as_arr().unwrap_or_default() {
            if let V::Obj(t) = term {
                let begin = source(&get(&t, "begin")).unwrap_or_default();
                mm.add_rule(begin, RuleType::Begin(t.clone()));
            }
        }
        let terminator = get(mode, "terminatorEnd");
        if terminator.truthy() {
            mm.add_rule(source(&terminator).unwrap_or_default(), RuleType::End);
        }
        let illegal = get(mode, "illegal");
        if illegal.truthy() {
            mm.add_rule(source(&illegal).unwrap_or_default(), RuleType::Illegal);
        }
        mm
    }

    /// `compileLanguage`.
    pub fn compile_language(&self) {
        let lang = &self.language;
        let aliases = match get(lang, "classNameAliases") {
            V::Obj(o) => inherit(&o, &[]),
            _ => new_obj(),
        };
        set(lang, "classNameAliases", V::Obj(aliases));
        self.compile_mode(lang, None);
    }
}

enum Expanded {
    One(Obj),
    Many(Vec<V>),
}

fn dependency_on_parent(mode: &V) -> bool {
    match mode {
        V::Obj(o) => get(o, "endsWithParent").truthy() || dependency_on_parent(&get(o, "starts")),
        _ => false,
    }
}

/// `expandOrCloneMode`.
fn expand_or_clone_mode(mode: &Obj) -> Expanded {
    let variants = get(mode, "variants");
    if variants.truthy() && !get(mode, "cachedVariants").truthy() {
        let variants_obj = new_obj();
        set(&variants_obj, "variants", V::Null);
        let cached: Vec<V> = variants
            .as_arr()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_obj().cloned())
            .map(|variant| V::Obj(inherit(mode, &[&variants_obj, &variant])))
            .collect();
        set(mode, "cachedVariants", new_arr(cached));
    }
    if let V::Arr(cached) = get(mode, "cachedVariants") {
        return Expanded::Many(cached.borrow().clone());
    }
    if dependency_on_parent(&V::Obj(mode.clone())) {
        let starts = match get(mode, "starts") {
            V::Obj(s) => V::Obj(inherit(&s, &[])),
            _ => V::Null,
        };
        let over = new_obj();
        set(&over, "starts", starts);
        return Expanded::One(inherit(mode, &[&over]));
    }
    if mode.borrow().frozen {
        return Expanded::One(inherit(mode, &[]));
    }
    Expanded::One(mode.clone())
}

// ---- token tree ------------------------------------------------------------

/// A node of the token tree: text, or a scope with children. A sub-language
/// node carries the language name as its kind (or none for plain text).
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text(String),
    Scope {
        kind: Option<String>,
        sublanguage: bool,
        children: Vec<Node>,
    },
}

/// `TokenTreeEmitter`.
#[derive(Debug, Clone)]
pub struct Emitter {
    root: Vec<Node>,
    /// Path of child indexes from the root to the open node.
    stack: Vec<usize>,
}

impl Emitter {
    fn new() -> Self {
        Self {
            root: Vec::new(),
            stack: Vec::new(),
        }
    }

    fn top_children(&mut self) -> &mut Vec<Node> {
        let mut children = &mut self.root;
        for &i in &self.stack {
            children = match &mut children[i] {
                Node::Scope { children, .. } => children,
                Node::Text(_) => unreachable!("text nodes are never open"),
            };
        }
        children
    }

    fn add(&mut self, node: Node) {
        self.top_children().push(node);
    }

    fn open_node(&mut self, kind: &str) {
        let children = self.top_children();
        children.push(Node::Scope {
            kind: Some(kind.to_string()),
            sublanguage: false,
            children: Vec::new(),
        });
        let i = children.len() - 1;
        self.stack.push(i);
    }

    fn close_node(&mut self) -> bool {
        self.stack.pop().is_some()
    }

    fn close_all_nodes(&mut self) {
        while self.close_node() {}
    }

    fn add_keyword(&mut self, text: &str, kind: &str) {
        if text.is_empty() {
            return;
        }
        self.open_node(kind);
        self.add_text(text);
        self.close_node();
    }

    fn add_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.add(Node::Text(text.to_string()));
    }

    fn add_sublanguage(&mut self, emitter: Emitter, name: Option<&str>) {
        self.add(Node::Scope {
            kind: name.map(String::from),
            sublanguage: true,
            children: emitter.root,
        });
    }

    pub fn into_nodes(self) -> Vec<Node> {
        self.root
    }
}

// ---- highlighting ----------------------------------------------------------

/// A mode on the stack (`Object.create(mode, { parent })`), or the language
/// itself at the root.
struct Top {
    mode: Obj,
    parent: Option<Rc<Top>>,
    is_root: bool,
}

pub struct HighlightResult {
    pub relevance: f64,
    pub language: Option<String>,
    pub illegal: bool,
    pub emitter: Emitter,
    /// The code, when the result is plain (an error or an illegal lexeme).
    pub plain: bool,
    top: Option<Rc<Top>>,
}

pub trait Registry {
    fn language(&self, name: &str) -> Option<(Obj, Vec<Callback>)>;
    fn language_names(&self) -> Vec<String>;
    fn system_symbols(&self) -> &std::collections::HashSet<String>;
}

struct Illegal;

pub fn highlight_with(
    registry: &dyn Registry,
    language_name: &str,
    code: &str,
    ignore_illegals: bool,
) -> Option<HighlightResult> {
    highlight_inner(registry, language_name, code, ignore_illegals, None)
}

fn highlight_inner(
    registry: &dyn Registry,
    language_name: &str,
    code: &str,
    ignore_illegals: bool,
    continuation: Option<Rc<Top>>,
) -> Option<HighlightResult> {
    let (language, extensions) = registry.language(language_name)?;
    let case_insensitive = get(&language, "case_insensitive").truthy();
    let compiler = Compiler {
        language: language.clone(),
        case_insensitive,
        extensions: &extensions,
    };
    compiler.compile_language();

    let mut h = Highlighter {
        registry,
        language: language.clone(),
        code,
        ignore_illegals,
        case_insensitive,
        top: continuation.unwrap_or_else(|| {
            Rc::new(Top {
                mode: language.clone(),
                parent: None,
                is_root: true,
            })
        }),
        continuations: HashMap::new(),
        emitter: Emitter::new(),
        mode_buffer: String::new(),
        relevance: 0.0,
        resume_scan_at_same_position: false,
        last_match: None,
    };
    h.process_continuations();
    let outcome = h.run();
    let top = h.top.clone();
    match outcome {
        Ok(()) => {
            h.emitter.close_all_nodes();
            Some(HighlightResult {
                relevance: h.relevance.floor(),
                language: Some(language_name.to_string()),
                illegal: false,
                emitter: h.emitter,
                plain: false,
                top: Some(top),
            })
        }
        Err(Stop::Illegal) => Some(HighlightResult {
            relevance: 0.0,
            language: None,
            illegal: true,
            emitter: h.emitter,
            plain: true,
            top: None,
        }),
        Err(Stop::Error) => Some(HighlightResult {
            relevance: 0.0,
            language: Some(language_name.to_string()),
            illegal: false,
            emitter: h.emitter,
            plain: true,
            top: Some(top),
        }),
    }
}

enum Stop {
    Illegal,
    Error,
}

impl From<Illegal> for Stop {
    fn from(_: Illegal) -> Self {
        Stop::Illegal
    }
}

struct Highlighter<'a> {
    registry: &'a dyn Registry,
    language: Obj,
    code: &'a str,
    ignore_illegals: bool,
    case_insensitive: bool,
    top: Rc<Top>,
    continuations: HashMap<String, Rc<Top>>,
    emitter: Emitter,
    mode_buffer: String,
    relevance: f64,
    resume_scan_at_same_position: bool,
    last_match: Option<(bool, usize)>,
}

/// A mode property as seen through `top` (own props, then the mode's).
fn top_get(top: &Top, key: &str) -> V {
    get(&top.mode, key)
}

fn matcher_of(top: &Top) -> Rc<RefCell<ResumableMultiRegex>> {
    match top_get(top, "matcher") {
        V::Matcher(m) => m,
        _ => Rc::new(RefCell::new(ResumableMultiRegex::new("mg"))),
    }
}

fn compiled(v: &V) -> Option<Rc<CompiledRe>> {
    match v {
        V::Compiled(c) => Some(c.clone()),
        _ => None,
    }
}

/// `startsWith(re, lexeme)`: a match at index 0.
fn starts_with(re: &V, lexeme: &str) -> bool {
    compiled(re).is_some_and(|c| c.re.exec(lexeme).is_some_and(|m| m.index() == 0))
}

/// `Response`: `ignoreMatch()` and the mode's `data`.
struct Response {
    ignored: bool,
}

impl Highlighter<'_> {
    fn class_alias(&self, kind: &str) -> String {
        match get(&self.language, "classNameAliases") {
            V::Obj(a) => match get(&a, kind) {
                V::Str(s) => s.to_string(),
                _ => kind.to_string(),
            },
            _ => kind.to_string(),
        }
    }

    fn process_keywords(&mut self) {
        let keywords = top_get(&self.top, "keywords");
        let V::Obj(keywords) = keywords else {
            let buffer = std::mem::take(&mut self.mode_buffer);
            self.emitter.add_text(&buffer);
            return;
        };
        let Some(pattern) = compiled(&top_get(&self.top, "keywordPatternRe")) else {
            let buffer = std::mem::take(&mut self.mode_buffer);
            self.emitter.add_text(&buffer);
            return;
        };
        let buffer = std::mem::take(&mut self.mode_buffer);
        let mut last_index = 0;
        let mut buf = String::new();
        let mut pos = 0;
        while let Some(m) = pattern.re.exec_at(&buffer, pos) {
            buf.push_str(&buffer[last_index..m.index()]);
            let word = m.whole();
            let key = if self.case_insensitive {
                word.to_lowercase()
            } else {
                word.to_string()
            };
            match get(&keywords, &key) {
                V::Keyword(kind, keyword_relevance) => {
                    self.emitter.add_text(&buf);
                    buf.clear();
                    self.relevance += keyword_relevance;
                    if kind.starts_with('_') {
                        buf.push_str(word);
                    } else {
                        let class = self.class_alias(&kind);
                        self.emitter.add_keyword(word, &class);
                    }
                }
                _ => buf.push_str(word),
            }
            last_index = m.end();
            // A global regex's lastIndex after an empty match does not move
            // (`exec` would loop), but the keyword patterns never match empty.
            pos = if m.end() == m.index() {
                match buffer[m.end()..].chars().next() {
                    Some(c) => m.end() + c.len_utf8(),
                    None => break,
                }
            } else {
                m.end()
            };
        }
        buf.push_str(&buffer[last_index.min(buffer.len())..]);
        self.emitter.add_text(&buf);
    }

    fn process_sub_language(&mut self) {
        if self.mode_buffer.is_empty() {
            return;
        }
        let buffer = std::mem::take(&mut self.mode_buffer);
        let sub = top_get(&self.top, "subLanguage");
        let result = if let V::Str(name) = &sub {
            if self.registry.language(name).is_none() {
                self.emitter.add_text(&buffer);
                return;
            }
            let continuation = self.continuations.get(&**name).cloned();
            let result = highlight_inner(self.registry, name, &buffer, true, continuation);
            if let Some(r) = &result {
                if let Some(top) = &r.top {
                    self.continuations.insert(name.to_string(), top.clone());
                }
            }
            result
        } else {
            let subset: Vec<String> = sub
                .as_arr()
                .unwrap_or_default()
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            Some(highlight_auto(
                self.registry,
                &buffer,
                if subset.is_empty() {
                    None
                } else {
                    Some(subset)
                },
            ))
        };
        let Some(result) = result else {
            return;
        };
        if top_get(&self.top, "relevance").as_num().unwrap_or(0.0) > 0.0 {
            self.relevance += result.relevance;
        }
        // An error result still carries its partial emitter, as in the pin.
        self.emitter
            .add_sublanguage(result.emitter, result.language.as_deref());
    }

    fn process_buffer(&mut self) {
        if !matches!(top_get(&self.top, "subLanguage"), V::Undefined | V::Null) {
            self.process_sub_language();
        } else {
            self.process_keywords();
        }
        self.mode_buffer.clear();
    }

    fn start_new_mode(&mut self, mode: &Obj) {
        if let V::Str(class) = get(mode, "className") {
            if !class.is_empty() {
                let class = self.class_alias(&class);
                self.emitter.open_node(&class);
            }
        }
        self.top = Rc::new(Top {
            mode: mode.clone(),
            parent: Some(self.top.clone()),
            is_root: false,
        });
    }

    fn run_callback(&self, cb: Callback, m: &ScanMatch, mode: &Obj) -> Response {
        let mut resp = Response { ignored: false };
        if get(mode, "data").is_undefined() {
            set(mode, "data", V::Obj(new_obj()));
        }
        let data = get(mode, "data");
        match cb {
            Callback::EndSameAsBeginBegin => {
                if let V::Obj(d) = data {
                    set(
                        &d,
                        "_beginMatch",
                        m.groups
                            .get(1)
                            .cloned()
                            .flatten()
                            .map_or(V::Undefined, |s| str_v(&s)),
                    );
                }
            }
            Callback::EndSameAsBeginEnd => {
                if let V::Obj(d) = data {
                    let begin = get(&d, "_beginMatch");
                    let end = m.groups.get(1).cloned().flatten();
                    let same = match (&begin, &end) {
                        (V::Str(a), Some(b)) => **a == **b,
                        (V::Undefined, None) => true,
                        _ => false,
                    };
                    if !same {
                        resp.ignored = true;
                    }
                }
            }
            Callback::ShebangBegin => {
                if m.index != 0 {
                    resp.ignored = true;
                }
            }
            Callback::SkipIfHasPrecedingDot => {
                if self.code[..m.index].ends_with('.') {
                    resp.ignored = true;
                }
            }
            Callback::MathematicaSystemSymbol => {
                if !self.registry.system_symbols().contains(m.lexeme()) {
                    resp.ignored = true;
                }
            }
            Callback::JsxIsTrulyOpeningTag => {
                let after = m.index + m.lexeme().len();
                let next = self.code[after.min(self.code.len())..].chars().next();
                if next == Some('<') {
                    resp.ignored = true;
                } else if next == Some('>') {
                    // hasClosingTag: `</` + the tag name after `<`.
                    let tag = format!("</{}", &m.lexeme()[1.min(m.lexeme().len())..]);
                    if !self.code[after..].contains(&tag) {
                        resp.ignored = true;
                    }
                }
            }
            Callback::ExtBeforeMatch => {}
        }
        resp
    }

    fn end_of_mode(
        &self,
        top: &Rc<Top>,
        m: &ScanMatch,
        match_plus_remainder: &str,
    ) -> Option<Rc<Top>> {
        let mut matched = starts_with(&top_get(top, "endRe"), match_plus_remainder);
        if matched {
            if let V::Fn(cb) = top_get(top, "on:end") {
                if self.run_callback(cb, m, &top.mode).ignored {
                    matched = false;
                }
            }
            if matched {
                let mut mode = top.clone();
                while top_get(&mode, "endsParent").truthy() {
                    match &mode.parent {
                        Some(p) => mode = p.clone(),
                        None => break,
                    }
                }
                return Some(mode);
            }
        }
        if top_get(top, "endsWithParent").truthy() {
            if let Some(parent) = &top.parent {
                return self.end_of_mode(parent, m, match_plus_remainder);
            }
        }
        None
    }

    fn do_ignore(&mut self, lexeme: &str) -> usize {
        let matcher = matcher_of(&self.top);
        if matcher.borrow().regex_index == 0 {
            let first = lexeme.chars().next();
            if let Some(c) = first {
                self.mode_buffer.push(c);
                c.len_utf8()
            } else {
                1
            }
        } else {
            self.resume_scan_at_same_position = true;
            0
        }
    }

    fn do_begin_match(&mut self, m: &ScanMatch, new_mode: &Obj) -> usize {
        let lexeme = m.lexeme().to_string();
        for key in ["__beforeBegin", "on:begin"] {
            if let V::Fn(cb) = get(new_mode, key) {
                if self.run_callback(cb, m, new_mode).ignored {
                    return self.do_ignore(&lexeme);
                }
            }
        }
        if get(new_mode, "endSameAsBegin").truthy() {
            set(new_mode, "endRe", V::Compiled(escape_re(&lexeme)));
        }
        if get(new_mode, "skip").truthy() {
            self.mode_buffer.push_str(&lexeme);
        } else {
            if get(new_mode, "excludeBegin").truthy() {
                self.mode_buffer.push_str(&lexeme);
            }
            self.process_buffer();
            if !get(new_mode, "returnBegin").truthy() && !get(new_mode, "excludeBegin").truthy() {
                self.mode_buffer = lexeme.clone();
            }
        }
        self.start_new_mode(new_mode);
        if get(new_mode, "returnBegin").truthy() {
            0
        } else {
            lexeme.len()
        }
    }

    /// `doEndMatch`; `None` is `NO_MATCH`.
    fn do_end_match(&mut self, m: &ScanMatch) -> Option<usize> {
        let lexeme = m.lexeme().to_string();
        let remainder = &self.code[m.index..];
        let end_mode = self.end_of_mode(&self.top.clone(), m, remainder)?;
        let origin = self.top.clone();
        if top_get(&origin, "skip").truthy() {
            self.mode_buffer.push_str(&lexeme);
        } else {
            if !(top_get(&origin, "returnEnd").truthy() || top_get(&origin, "excludeEnd").truthy())
            {
                self.mode_buffer.push_str(&lexeme);
            }
            self.process_buffer();
            if top_get(&origin, "excludeEnd").truthy() {
                self.mode_buffer = lexeme.clone();
            }
        }
        loop {
            let top = self.top.clone();
            if let V::Str(class) = top_get(&top, "className") {
                if !class.is_empty() {
                    self.emitter.close_node();
                }
            }
            if !top_get(&top, "skip").truthy() && !top_get(&top, "subLanguage").truthy() {
                self.relevance += top_get(&top, "relevance").as_num().unwrap_or(0.0);
            }
            let Some(parent) = top.parent.clone() else {
                break;
            };
            self.top = parent;
            let reached = match &end_mode.parent {
                Some(p) => Rc::ptr_eq(&self.top, p),
                None => false,
            };
            if reached {
                break;
            }
        }
        if let V::Obj(starts) = get(&end_mode.mode, "starts") {
            if get(&end_mode.mode, "endSameAsBegin").truthy() {
                set(&starts, "endRe", get(&end_mode.mode, "endRe"));
            }
            self.start_new_mode(&starts);
        }
        Some(if top_get(&origin, "returnEnd").truthy() {
            0
        } else {
            lexeme.len()
        })
    }

    fn process_continuations(&mut self) {
        let mut list = Vec::new();
        let mut current = Some(self.top.clone());
        while let Some(c) = current {
            if c.is_root || Rc::ptr_eq(&c.mode, &self.language) && c.parent.is_none() {
                break;
            }
            if let V::Str(class) = top_get(&c, "className") {
                if !class.is_empty() {
                    list.insert(0, class.to_string());
                }
            }
            current = c.parent.clone();
        }
        for item in list {
            self.emitter.open_node(&item);
        }
    }

    fn process_lexeme(
        &mut self,
        text_before: &str,
        m: Option<&ScanMatch>,
        index: usize,
    ) -> Result<usize, Stop> {
        self.mode_buffer.push_str(text_before);
        let Some(m) = m else {
            self.process_buffer();
            return Ok(0);
        };
        let lexeme = m.lexeme().to_string();
        let is_begin = matches!(m.kind, RuleType::Begin(_));
        let is_end = matches!(m.kind, RuleType::End);
        if let Some((last_begin, last_index)) = self.last_match {
            if last_begin && is_end && last_index == m.index && lexeme.is_empty() {
                // A zero-width end right after a zero-width begin: step a char.
                let c = self.code[m.index..].chars().next();
                if let Some(c) = c {
                    self.mode_buffer.push(c);
                    return Ok(c.len_utf8());
                }
                return Ok(1);
            }
        }
        self.last_match = Some((is_begin, m.index));
        match &m.kind {
            RuleType::Begin(mode) => return Ok(self.do_begin_match(m, &mode.clone())),
            RuleType::Illegal if !self.ignore_illegals => return Err(Stop::Illegal),
            RuleType::End => {
                if let Some(processed) = self.do_end_match(m) {
                    return Ok(processed);
                }
            }
            _ => {}
        }
        if matches!(m.kind, RuleType::Illegal) && lexeme.is_empty() {
            return Ok(1);
        }
        let _ = index;
        self.mode_buffer.push_str(&lexeme);
        Ok(lexeme.len())
    }

    fn run(&mut self) -> Result<(), Stop> {
        let mut index = 0usize;
        let mut iterations = 0usize;
        matcher_of(&self.top).borrow_mut().consider_all();
        loop {
            iterations += 1;
            let matcher = matcher_of(&self.top);
            if self.resume_scan_at_same_position {
                self.resume_scan_at_same_position = false;
            } else {
                matcher.borrow_mut().consider_all();
            }
            matcher.borrow_mut().last_index = index;
            let m = matcher.borrow_mut().exec(self.code);
            let Some(m) = m else { break };
            let before = &self.code[index.min(m.index)..m.index];
            let processed = self.process_lexeme(before, Some(&m), index)?;
            index = m.index + processed;
            // A step that lands inside a character (one-unit steps over
            // multi-byte text) moves on to the next boundary.
            while index < self.code.len() && !self.code.is_char_boundary(index) {
                index += 1;
            }
            if iterations > 100_000 && iterations > m.index * 3 {
                return Err(Stop::Error);
            }
        }
        let rest = &self.code[index.min(self.code.len())..];
        self.process_lexeme(rest, None, index)?;
        Ok(())
    }
}

/// `highlightAuto(code, languageSubset)`.
pub fn highlight_auto(
    registry: &dyn Registry,
    code: &str,
    subset: Option<Vec<String>>,
) -> HighlightResult {
    let names = subset.unwrap_or_else(|| registry.language_names());
    let mut plain = Emitter::new();
    plain.add_text(code);
    let mut results = vec![HighlightResult {
        relevance: 0.0,
        language: None,
        illegal: false,
        emitter: plain,
        plain: true,
        top: None,
    }];
    for name in names {
        let Some((lang, _)) = registry.language(&name) else {
            continue;
        };
        if get(&lang, "disableAutodetect").truthy() {
            continue;
        }
        if let Some(r) = highlight_inner(registry, &name, code, false, None) {
            results.push(r);
        }
    }
    // Stable sort by relevance, then the superset rule.
    let superset_of = |name: &Option<String>| -> Option<String> {
        name.as_deref()
            .and_then(|n| registry.language(n))
            .and_then(|(l, _)| get(&l, "supersetOf").as_str().map(String::from))
    };
    let mut indexed: Vec<(usize, HighlightResult)> = results.into_iter().enumerate().collect();
    indexed.sort_by(|(ia, a), (ib, b)| {
        use std::cmp::Ordering;
        if a.relevance != b.relevance {
            return b
                .relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(Ordering::Equal);
        }
        if a.language.is_some() && b.language.is_some() {
            if superset_of(&a.language) == b.language {
                return Ordering::Greater;
            } else if superset_of(&b.language) == a.language {
                return Ordering::Less;
            }
        }
        ia.cmp(ib)
    });
    indexed
        .into_iter()
        .next()
        .map(|(_, r)| r)
        .expect("plaintext result")
}
