//! The little JSON this crate needs, written out rather than pulled in.
//!
//! Emitting is a string escaper plus `format!`: every command here builds a flat,
//! known-shaped object, so a serialiser would be more machinery than the job asks
//! for.
//!
//! Reading is only needed by one command. `publish-readiness` has to ask cargo
//! what the workspace contains, and `cargo metadata` answers in JSON. Reading
//! that back with string searches would be guesswork, so there is a real parser
//! below, kept to the subset `cargo metadata` actually produces.

use std::fmt::Write as _;

/// Escapes a string for use inside a JSON string literal.
///
/// Carriage returns are dropped rather than escaped: the only strings that carry
/// them are captured process output, where a `\r\n` from a Windows child process
/// would otherwise show up as a literal `\r` in every line of the log tail.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {}
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Renders `s` as a complete JSON string, quotes included.
pub fn quote(s: &str) -> String {
    format!("\"{}\"", escape(s))
}

/// Keeps the last `n` lines of captured output, for the `log_tail` field.
///
/// The tail is what makes a JSON result actionable: without it a failure reports
/// only that something failed. It is bounded because these results are read by
/// an agent with a context budget.
pub fn log_tail(log: &str, n: usize) -> String {
    let lines: Vec<&str> = log.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// A parsed JSON value.
///
/// Objects keep their keys in a `Vec` rather than a map: the documents read here
/// are small, lookups are few, and preserving order keeps the parser honest about
/// what it saw.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value under `key`, or `None` if this is not an object or has no such key.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// True for JSON `null`, and for a value that is absent entirely.
    ///
    /// `cargo metadata` distinguishes "not restricted" (`null`) from "restricted
    /// to this list of registries" (an array), and `publish = false` is the empty
    /// array. Reading that correctly needs `null` to be its own answer rather
    /// than folding into "missing".
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// True when a field carries no information: `null`, `""`, or `[]`.
    ///
    /// This is the test for "the crate did not declare this", which is what the
    /// publish-metadata check is actually asking.
    pub fn is_blank(&self) -> bool {
        match self {
            Value::Null => true,
            Value::String(s) => s.is_empty(),
            Value::Array(items) => items.is_empty(),
            _ => false,
        }
    }
}

/// Parses a complete JSON document.
///
/// Returns the offset-tagged reason on failure. The caller turns that into a
/// message naming what it was reading, because "unexpected token at 12043" is
/// only useful with that context.
pub fn parse(input: &str) -> Result<Value, String> {
    let bytes = input.as_bytes();
    let mut p = Parser { bytes, pos: 0 };
    p.skip_whitespace();
    let value = p.value()?;
    p.skip_whitespace();
    if p.pos != bytes.len() {
        return Err(format!("trailing content at byte {}", p.pos));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_whitespace(&mut self) {
        while matches!(self.bytes.get(self.pos), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), String> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(format!("expected `{}` at byte {}", byte as char, self.pos))
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(format!("expected `{word}` at byte {}", self.pos))
        }
    }

    fn value(&mut self) -> Result<Value, String> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Value::String(self.string()?)),
            Some(b't') => self.literal("true", Value::Bool(true)),
            Some(b'f') => self.literal("false", Value::Bool(false)),
            Some(b'n') => self.literal("null", Value::Null),
            Some(_) => self.number(),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn object(&mut self) -> Result<Value, String> {
        self.expect(b'{')?;
        let mut fields = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Value::Object(fields));
        }
        loop {
            self.skip_whitespace();
            let key = self.string()?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            fields.push((key, self.value()?));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Value::Object(fields));
                }
                _ => return Err(format!("expected `,` or `}}` at byte {}", self.pos)),
            }
        }
    }

    fn array(&mut self) -> Result<Value, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Value::Array(items));
        }
        loop {
            self.skip_whitespace();
            items.push(self.value()?);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Value::Array(items));
                }
                _ => return Err(format!("expected `,` or `]` at byte {}", self.pos)),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self
                .peek()
                .ok_or_else(|| "unterminated string".to_string())?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let escaped = self
                        .peek()
                        .ok_or_else(|| "unterminated escape".to_string())?;
                    self.pos += 1;
                    match escaped {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        other => {
                            return Err(format!(
                                "unknown escape `\\{}` at byte {}",
                                other as char, self.pos
                            ));
                        }
                    }
                }
                _ => {
                    // Copy the whole UTF-8 sequence, not one byte: paths and
                    // descriptions in cargo metadata are not all ASCII.
                    let start = self.pos;
                    self.pos += 1;
                    while self.peek().is_some_and(|b| (0x80..0xC0).contains(&b)) {
                        self.pos += 1;
                    }
                    out.push_str(
                        std::str::from_utf8(&self.bytes[start..self.pos])
                            .map_err(|_| format!("invalid UTF-8 at byte {start}"))?,
                    );
                }
            }
        }
    }

    /// Reads the four hex digits after `\u`, pairing surrogates when it must.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let high = self.hex4()?;
        if (0xD800..0xDC00).contains(&high) {
            if self.bytes[self.pos..].starts_with(b"\\u") {
                self.pos += 2;
                let low = self.hex4()?;
                let combined = 0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                return char::from_u32(combined)
                    .ok_or_else(|| format!("invalid surrogate pair at byte {}", self.pos));
            }
            return Err(format!("lone surrogate at byte {}", self.pos));
        }
        char::from_u32(high).ok_or_else(|| format!("invalid escape at byte {}", self.pos))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let end = self.pos + 4;
        let slice = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| "truncated \\u escape".to_string())?;
        let text = std::str::from_utf8(slice).map_err(|_| "invalid \\u escape".to_string())?;
        let value = u32::from_str_radix(text, 16)
            .map_err(|_| format!("invalid \\u escape at byte {}", self.pos))?;
        self.pos = end;
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|b| matches!(b, b'-' | b'+' | b'.' | b'0'..=b'9' | b'e' | b'E'))
        {
            self.pos += 1;
        }
        if start == self.pos {
            return Err(format!("expected a value at byte {start}"));
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| "invalid number".to_string())?;
        text.parse::<f64>()
            .map(Value::Number)
            .map_err(|_| format!("invalid number `{text}` at byte {start}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_should_encode_the_characters_json_forbids() {
        // The carriage return is dropped, so `e` and `f` end up adjacent.
        assert_eq!(escape("a\"b\\c\nd\te\rf"), "a\\\"b\\\\c\\nd\\tef");
        assert_eq!(escape("\u{1}"), "\\u0001");
    }

    #[test]
    fn parse_should_read_the_shapes_cargo_metadata_produces() {
        let doc = r#"{"packages":[{"name":"avio","publish":null,"keywords":["video"],
                      "license":"MIT","description":"","nested":{"n":-1.5e2,"ok":true}}]}"#;
        let value = parse(doc).expect("valid document");
        let package = &value.get("packages").unwrap().as_array().unwrap()[0];
        assert_eq!(package.get("name").unwrap().as_str(), Some("avio"));
        assert!(package.get("publish").unwrap().is_null());
        assert!(!package.get("keywords").unwrap().is_blank());
        assert!(package.get("description").unwrap().is_blank());
        assert_eq!(
            package.get("nested").unwrap().get("n"),
            Some(&Value::Number(-150.0))
        );
    }

    #[test]
    fn parse_should_decode_escapes_and_non_ascii() {
        let value = parse(r#"{"path":"C:\\tmp\u0041","text":"caf\u00e9 \ud83d\ude00"}"#)
            .expect("valid document");
        assert_eq!(value.get("path").unwrap().as_str(), Some("C:\\tmpA"));
        assert_eq!(value.get("text").unwrap().as_str(), Some("café 😀"));
    }

    #[test]
    fn parse_should_reject_a_truncated_document() {
        assert!(parse(r#"{"a":"#).is_err());
        assert!(parse(r#"{"a":1} {"b":2}"#).is_err());
    }

    #[test]
    fn log_tail_should_keep_only_the_last_lines() {
        let log = (1..=10)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(log_tail(&log, 3), "8\n9\n10");
        assert_eq!(log_tail(&log, 50), log);
    }
}
