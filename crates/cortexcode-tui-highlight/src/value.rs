//! A small model of the JavaScript values highlight.js grammars are made of.
//!
//! highlight.js compiles a language by mutating its mode objects in place,
//! and modes and keyword tables are shared between parents; the compiled
//! result depends on that sharing (a keywords object's `$pattern` is deleted
//! by the first mode that compiles it). So grammars are kept as shared,
//! mutable objects with JavaScript's identity, truthiness and frozen-object
//! rules, and the engine is ported against them.

use std::cell::RefCell;
use std::rc::Rc;

use cortexcode_tui_util::js_regex::JsRegex;

use crate::engine::ResumableMultiRegex;

/// A named callback (`on:begin`, `on:end`, a compiler extension).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Callback {
    EndSameAsBeginBegin,
    EndSameAsBeginEnd,
    ShebangBegin,
    JsxIsTrulyOpeningTag,
    MathematicaSystemSymbol,
    SkipIfHasPrecedingDot,
    ExtBeforeMatch,
}

impl Callback {
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "endSameAsBegin.begin" => Self::EndSameAsBeginBegin,
            "endSameAsBegin.end" => Self::EndSameAsBeginEnd,
            "shebang.begin" => Self::ShebangBegin,
            "jsx.isTrulyOpeningTag" => Self::JsxIsTrulyOpeningTag,
            "mathematica.systemSymbol" => Self::MathematicaSystemSymbol,
            "ext.beforeMatch" => Self::ExtBeforeMatch,
            _ => return None,
        })
    }
}

/// A regex literal: its source and flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReLit {
    pub source: String,
    pub flags: String,
}

/// A compiled regex with the source it came from.
pub struct CompiledRe {
    pub source: String,
    pub re: JsRegex,
}

impl std::fmt::Debug for CompiledRe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "/{}/", self.source)
    }
}

pub type Obj = Rc<RefCell<ObjData>>;
pub type Arr = Rc<RefCell<Vec<V>>>;

#[derive(Clone, Debug)]
pub enum V {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(Rc<str>),
    Re(Rc<ReLit>),
    Arr(Arr),
    Obj(Obj),
    Fn(Callback),
    /// A compiled regex the engine stored on a mode.
    Compiled(Rc<CompiledRe>),
    /// A mode's scanner.
    Matcher(Rc<RefCell<ResumableMultiRegex>>),
    /// A compiled keyword: `[className, relevance]`.
    Keyword(Rc<str>, f64),
}

#[derive(Debug, Default)]
pub struct ObjData {
    props: Vec<(String, V)>,
    pub frozen: bool,
}

impl ObjData {
    pub fn keys(&self) -> Vec<String> {
        self.props.iter().map(|(k, _)| k.clone()).collect()
    }

    pub fn entries(&self) -> Vec<(String, V)> {
        self.props.clone()
    }
}

pub fn new_obj() -> Obj {
    Rc::new(RefCell::new(ObjData::default()))
}

pub fn new_arr(items: Vec<V>) -> V {
    V::Arr(Rc::new(RefCell::new(items)))
}

/// `obj[key]`.
pub fn get(obj: &Obj, key: &str) -> V {
    obj.borrow()
        .props
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or(V::Undefined)
}

/// `key in obj` / `hasOwnProperty`.
pub fn has(obj: &Obj, key: &str) -> bool {
    obj.borrow().props.iter().any(|(k, _)| k == key)
}

/// `obj[key] = value` (ignored on a frozen object, as in sloppy mode).
pub fn set(obj: &Obj, key: &str, value: V) {
    let mut o = obj.borrow_mut();
    if o.frozen {
        return;
    }
    if let Some(slot) = o.props.iter_mut().find(|(k, _)| k == key) {
        slot.1 = value;
    } else {
        o.props.push((key.to_string(), value));
    }
}

/// `delete obj[key]` (ignored on a frozen object).
pub fn delete(obj: &Obj, key: &str) {
    let mut o = obj.borrow_mut();
    if o.frozen {
        return;
    }
    o.props.retain(|(k, _)| k != key);
}

/// `inherit(original, ...objects)`: a fresh object with every key copied.
pub fn inherit(original: &Obj, others: &[&Obj]) -> Obj {
    let result = new_obj();
    for (k, v) in original.borrow().props.iter() {
        set(&result, k, v.clone());
    }
    for other in others {
        for (k, v) in other.borrow().props.iter() {
            set(&result, k, v.clone());
        }
    }
    result
}

impl V {
    /// JavaScript truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            V::Undefined | V::Null => false,
            V::Bool(b) => *b,
            V::Num(n) => *n != 0.0 && !n.is_nan(),
            V::Str(s) => !s.is_empty(),
            _ => true,
        }
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, V::Undefined)
    }

    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            V::Obj(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_num(&self) -> Option<f64> {
        match self {
            V::Num(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_arr(&self) -> Option<Vec<V>> {
        match self {
            V::Arr(a) => Some(a.borrow().clone()),
            _ => None,
        }
    }

    /// `typeof value === "object"` (arrays, objects and `null`).
    pub fn is_js_object(&self) -> bool {
        matches!(self, V::Arr(_) | V::Obj(_) | V::Null)
    }

    /// Object identity (`===` for objects).
    pub fn same(&self, other: &V) -> bool {
        match (self, other) {
            (V::Obj(a), V::Obj(b)) => Rc::ptr_eq(a, b),
            (V::Arr(a), V::Arr(b)) => Rc::ptr_eq(a, b),
            (V::Str(a), V::Str(b)) => a == b,
            (V::Undefined, V::Undefined) | (V::Null, V::Null) => true,
            _ => false,
        }
    }
}

pub fn str_v(s: &str) -> V {
    V::Str(Rc::from(s))
}

// ---- loading the dump ------------------------------------------------------

/// The grammars, as `hljs-grammars.mjs` dumped them.
pub struct Grammars {
    /// Language name -> its definition object.
    pub languages: Vec<(String, Obj)>,
    pub aliases: Vec<(String, String)>,
    pub system_symbols: std::collections::HashSet<String>,
}

fn decode(value: &serde_json::Value, nodes: &[Obj]) -> V {
    use serde_json::Value as J;
    match value {
        J::Null => V::Null,
        J::Bool(b) => V::Bool(*b),
        J::Number(n) => V::Num(n.as_f64().unwrap_or(f64::NAN)),
        J::String(s) => str_v(s),
        J::Array(items) => new_arr(items.iter().map(|v| decode(v, nodes)).collect()),
        J::Object(map) => {
            if let Some(id) = map.get("$ref").and_then(J::as_u64) {
                return V::Obj(nodes[id as usize].clone());
            }
            if let Some(items) = map.get("$arr").and_then(J::as_array) {
                return new_arr(items.iter().map(|v| decode(v, nodes)).collect());
            }
            if let Some(source) = map.get("$re").and_then(J::as_str) {
                return V::Re(Rc::new(ReLit {
                    source: source.to_string(),
                    flags: map
                        .get("flags")
                        .and_then(J::as_str)
                        .unwrap_or("")
                        .to_string(),
                }));
            }
            if let Some(name) = map.get("$fn").and_then(J::as_str) {
                return match Callback::from_name(name) {
                    Some(cb) => V::Fn(cb),
                    None => V::Undefined,
                };
            }
            V::Undefined
        }
    }
}

pub fn load(json: &str) -> Grammars {
    let doc: serde_json::Value = serde_json::from_str(json).expect("grammar dump");
    let raw_nodes = doc["nodes"].as_array().expect("nodes");
    let nodes: Vec<Obj> = raw_nodes.iter().map(|_| new_obj()).collect();
    for (node, raw) in nodes.iter().zip(raw_nodes) {
        let mut props = Vec::new();
        for (k, v) in raw["props"].as_object().expect("props") {
            let value = decode(v, &nodes);
            // JSON has no `undefined`; the dumper wrote it as null, and
            // `undefined`-valued keys behave as absent for the engine.
            if matches!(value, V::Null) && v.is_null() {
                props.push((k.clone(), V::Undefined));
                continue;
            }
            props.push((k.clone(), value));
        }
        let mut data = node.borrow_mut();
        data.props = props;
        data.frozen = raw["frozen"].as_bool().unwrap_or(false);
    }
    let languages = doc["languages"]
        .as_object()
        .expect("languages")
        .iter()
        .map(|(name, v)| {
            let obj = decode(v, &nodes)
                .as_obj()
                .cloned()
                .expect("language object");
            (name.clone(), obj)
        })
        .collect();
    let aliases = doc["aliases"]
        .as_object()
        .expect("aliases")
        .iter()
        .map(|(a, n)| (a.clone(), n.as_str().unwrap_or("").to_string()))
        .collect();
    let system_symbols = doc["systemSymbols"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    Grammars {
        languages,
        aliases,
        system_symbols,
    }
}
