//! Incomplete-JSON parser.
//!
//! Port of the `partial-json` npm package (0.1.7, `Allow.ALL`) that hoocode's
//! `parseStreamingJson` (`utils/json-parse.ts`) falls back to while a tool
//! call's arguments are still streaming: `{"path":"READ` parses as
//! `{"path":"READ"}`, `[1,2` as `[1,2]`, `tr` as `true`. `Infinity` and `NaN`
//! have no JSON value and parse as `null`.

use serde_json::{Map, Value};

/// Why [`parse_partial_json`] gave up (`PartialJSON` / `MalformedJSON`).
#[derive(Debug, Clone, PartialEq)]
pub struct PartialJsonError(pub String);

/// `parse(jsonString)` with every partial type allowed.
pub fn parse_partial_json(json: &str) -> Result<Value, PartialJsonError> {
    let trimmed = json.trim();
    if trimmed.is_empty() {
        return Err(PartialJsonError(format!("{json} is empty")));
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let mut parser = Parser {
        s: &chars,
        index: 0,
    };
    parser.parse_any()
}

struct Parser<'a> {
    s: &'a [char],
    index: usize,
}

fn js_json_parse(text: &str) -> Result<Value, PartialJsonError> {
    serde_json::from_str::<Value>(text).map_err(|e| PartialJsonError(e.to_string()))
}

impl Parser<'_> {
    fn len(&self) -> usize {
        self.s.len()
    }

    fn at(&self, i: usize) -> Option<char> {
        self.s.get(i).copied()
    }

    /// JS `substring(a, b)`: clamped to the string, arguments swapped when
    /// `b < a`.
    fn substring(&self, a: isize, b: isize) -> String {
        let len = self.len() as isize;
        let (a, b) = (a.clamp(0, len), b.clamp(0, len));
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        self.s[lo as usize..hi as usize].iter().collect()
    }

    fn last_index_of(&self, c: char) -> isize {
        self.s
            .iter()
            .rposition(|&x| x == c)
            .map_or(-1, |i| i as isize)
    }

    fn partial(&self, msg: &str) -> PartialJsonError {
        PartialJsonError(format!("{msg} at position {}", self.index))
    }

    fn rest_is_prefix_of(&self, word: &str, max_rest: usize) -> bool {
        let rest: String = self.s[self.index.min(self.len())..].iter().collect();
        let remaining = self.len().saturating_sub(self.index);
        remaining < max_rest && word.starts_with(&rest)
    }

    fn starts_with_word(&self, word: &str) -> bool {
        let end = (self.index + word.chars().count()).min(self.len());
        self.s[self.index.min(self.len())..end]
            .iter()
            .collect::<String>()
            == word
    }

    fn parse_any(&mut self) -> Result<Value, PartialJsonError> {
        self.skip_blank();
        if self.index >= self.len() {
            return Err(self.partial("Unexpected end of input"));
        }
        match self.at(self.index) {
            Some('"') => return self.parse_str().map(Value::String),
            Some('{') => return self.parse_obj(),
            Some('[') => return self.parse_arr(),
            _ => {}
        }
        for (word, value) in [
            ("null", Value::Null),
            ("true", Value::Bool(true)),
            ("false", Value::Bool(false)),
            ("Infinity", Value::Null),
        ] {
            let n = word.len();
            if self.starts_with_word(word) || self.rest_is_prefix_of(word, n) {
                self.index += n;
                return Ok(value);
            }
        }
        let remaining = self.len() - self.index;
        if self.starts_with_word("-Infinity")
            || (1 < remaining && self.rest_is_prefix_of("-Infinity", 9))
        {
            self.index += 9;
            return Ok(Value::Null);
        }
        if self.starts_with_word("NaN") || self.rest_is_prefix_of("NaN", 3) {
            self.index += 3;
            return Ok(Value::Null);
        }
        self.parse_num()
    }

    fn parse_str(&mut self) -> Result<String, PartialJsonError> {
        let start = self.index;
        let mut escape = false;
        self.index += 1; // skip the opening quote
        while self.index < self.len()
            && (self.at(self.index) != Some('"')
                || (escape && self.at(self.index - 1) == Some('\\')))
        {
            escape = if self.at(self.index) == Some('\\') {
                !escape
            } else {
                false
            };
            self.index += 1;
        }
        let as_string = |v: Value| match v {
            Value::String(s) => Ok(s),
            other => Err(PartialJsonError(format!("expected a string, got {other}"))),
        };
        if self.at(self.index) == Some('"') {
            self.index += 1;
            let end = self.index as isize - isize::from(escape);
            return js_json_parse(&self.substring(start as isize, end)).and_then(as_string);
        }
        // Unterminated: close it; on an invalid escape, cut at the last `\`.
        let end = self.index as isize - isize::from(escape);
        let text = format!("{}\"", self.substring(start as isize, end));
        match js_json_parse(&text) {
            Ok(v) => as_string(v),
            Err(_) => {
                let cut = self.last_index_of('\\');
                let text = format!("{}\"", self.substring(start as isize, cut));
                js_json_parse(&text).and_then(as_string)
            }
        }
    }

    fn parse_obj(&mut self) -> Result<Value, PartialJsonError> {
        self.index += 1; // skip the opening brace
        self.skip_blank();
        let mut obj = Map::new();
        let complete = (|| -> Result<(), PartialJsonError> {
            while self.at(self.index) != Some('}') {
                self.skip_blank();
                if self.index >= self.len() {
                    return Err(self.partial("Unterminated object"));
                }
                let key = self.parse_str()?;
                self.skip_blank();
                self.index += 1; // skip the colon
                let value = self.parse_any()?;
                obj.insert(key, value);
                self.skip_blank();
                if self.at(self.index) == Some(',') {
                    self.index += 1;
                }
            }
            Ok(())
        })();
        if complete.is_ok() {
            self.index += 1; // skip the closing brace
        }
        // Allow.OBJ: whatever was read so far.
        Ok(Value::Object(obj))
    }

    fn parse_arr(&mut self) -> Result<Value, PartialJsonError> {
        self.index += 1; // skip the opening bracket
        let mut arr = Vec::new();
        let complete = (|| -> Result<(), PartialJsonError> {
            while self.at(self.index) != Some(']') {
                arr.push(self.parse_any()?);
                self.skip_blank();
                if self.at(self.index) == Some(',') {
                    self.index += 1;
                }
            }
            Ok(())
        })();
        if complete.is_ok() {
            self.index += 1; // skip the closing bracket
        }
        // Allow.ARR: whatever was read so far.
        Ok(Value::Array(arr))
    }

    fn parse_num(&mut self) -> Result<Value, PartialJsonError> {
        if self.index == 0 {
            let whole: String = self.s.iter().collect();
            if whole == "-" {
                return Err(PartialJsonError("Not sure what '-' is".into()));
            }
            return js_json_parse(&whole).or_else(|e| {
                js_json_parse(&self.substring(0, self.last_index_of('e'))).map_err(|_| e)
            });
        }
        let start = self.index;
        if self.at(self.index) == Some('-') {
            self.index += 1;
        }
        while let Some(c) = self.at(self.index) {
            if ",]}".contains(c) {
                break;
            }
            self.index += 1;
        }
        let text = self.substring(start as isize, self.index as isize);
        js_json_parse(&text).or_else(|e| {
            if text == "-" {
                return Err(self.partial("Not sure what '-' is"));
            }
            js_json_parse(&self.substring(start as isize, self.last_index_of('e'))).map_err(|_| e)
        })
    }

    fn skip_blank(&mut self) {
        while self.index < self.len() && " \n\r\t".contains(self.s[self.index]) {
            self.index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_the_partial_json_package() {
        // Recorded with partial-json 0.1.7 at the hoocode pin.
        let cases = [
            (r#"{"path":"README"#, json!({"path": "README"})),
            (r#"{"a":1,"b":[1,2"#, json!({"a": 1, "b": [1, 2]})),
            (r#"{"a":{"b":"c"#, json!({"a": {"b": "c"}})),
            (r#"{"a":tr"#, json!({"a": true})),
            (r#"{"a":1."#, json!({})),
            (r#"{"a":"x\"#, json!({"a": "x"})),
            (r#"[1,2,{"k":"#, json!([1, 2, {}])),
            (r#"{"a":nu"#, json!({"a": null})),
            (r#"{"a"#, json!({})),
            (r#"{"a":"#, json!({})),
            (r#""str"#, json!("str")),
            (r#"{"a":-"#, json!({})),
            (r#"{"a":"\u00"#, json!({"a": ""})),
            (
                r#"{"a": [1, 2], "b": fal"#,
                json!({"a": [1, 2], "b": false}),
            ),
            (r#"  {"a": "x"}  "#, json!({"a": "x"})),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_partial_json(input).ok(), Some(expected), "{input}");
        }
        assert!(parse_partial_json("   ").is_err());
        assert!(parse_partial_json("-").is_err());
    }
}
