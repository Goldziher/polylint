//! A JSON and JSONC parser that records the source span of every value.
//!
//! `serde_json` keeps no positions, and the validator reports only a JSON Pointer, so this parser
//! builds the `serde_json::Value` itself and remembers where each pointer sits. Comments and
//! trailing commas are accepted, which is JSONC; strictness is the formatter and linter's job.

use std::collections::HashMap;

use poly_engine_api::engine::Span;
use poly_engine_api::line_index::LineIndex;
use serde_json::{Map, Value};

use crate::doc::{Document, MAX_DEPTH, child_if, element_if, span_between};

/// Parses `src` into its value and spans.
///
/// # Errors
///
/// A syntax error with its `line:column`.
pub(crate) fn parse(src: &str, record: bool) -> Result<Document, String> {
    let mut parser = Parser {
        src,
        bytes: src.as_bytes(),
        pos: if src.starts_with('\u{feff}') {
            '\u{feff}'.len_utf8()
        } else {
            0
        },
        lines: LineIndex::new(src),
        spans: HashMap::new(),
        record,
    };
    parser.skip()?;
    let value = parser.value("", 0)?;
    parser.skip()?;
    if parser.pos < parser.bytes.len() {
        return Err(parser.error("unexpected content after the document"));
    }
    Ok(if record {
        Document::new(value, parser.spans)
    } else {
        Document::unrecorded(value)
    })
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
    lines: LineIndex<'a>,
    spans: HashMap<String, Span>,
    record: bool,
}

impl Parser<'_> {
    fn error(&self, what: &str) -> String {
        let (line, col) = self.lines.line_col(self.pos);
        format!("invalid JSON: {what} at {line}:{col}")
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    /// Skips whitespace and `//` and `/* */` comments.
    fn skip(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => self.pos += 1,
                Some(b'/') => match self.bytes.get(self.pos + 1) {
                    Some(b'/') => {
                        while self.peek().is_some_and(|byte| byte != b'\n') {
                            self.pos += 1;
                        }
                    }
                    Some(b'*') => match self.src[self.pos + 2..].find("*/") {
                        Some(end) => self.pos += 2 + end + 2,
                        None => return Err(self.error("unterminated comment")),
                    },
                    _ => return Ok(()),
                },
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self, pointer: &str, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nesting is too deep"));
        }
        let start = self.pos;
        let value = match self.peek() {
            Some(b'{') => self.object(pointer, depth)?,
            Some(b'[') => self.array(pointer, depth)?,
            Some(b'"') => Value::String(self.string()?),
            Some(b'-' | b'0'..=b'9') => self.number()?,
            Some(b't') => self.literal("true", Value::Bool(true))?,
            Some(b'f') => self.literal("false", Value::Bool(false))?,
            Some(b'n') => self.literal("null", Value::Null)?,
            _ => return Err(self.error("expected a value")),
        };
        if self.record {
            let span = span_between(&self.lines, start, self.pos);
            self.spans.insert(pointer.to_owned(), span);
        }
        Ok(value)
    }

    fn object(&mut self, pointer: &str, depth: usize) -> Result<Value, String> {
        self.pos += 1;
        let mut members = Map::new();
        loop {
            self.skip()?;
            match self.peek() {
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                Some(b'"') => {}
                _ => return Err(self.error("expected a string key or `}`")),
            }
            let key = self.string()?;
            self.skip()?;
            if self.peek() != Some(b':') {
                return Err(self.error("expected `:`"));
            }
            self.pos += 1;
            self.skip()?;
            let path = child_if(self.record, pointer, &key);
            let value = self.value(&path, depth + 1)?;
            members.insert(key, value);
            self.skip()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
        Ok(Value::Object(members))
    }

    fn array(&mut self, pointer: &str, depth: usize) -> Result<Value, String> {
        self.pos += 1;
        let mut items = Vec::new();
        loop {
            self.skip()?;
            if self.peek() == Some(b']') {
                self.pos += 1;
                break;
            }
            let path = element_if(self.record, pointer, items.len());
            let value = self.value(&path, depth + 1)?;
            items.push(value);
            self.skip()?;
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
        Ok(Value::Array(items))
    }

    fn string(&mut self) -> Result<String, String> {
        self.pos += 1;
        let mut out = String::new();
        loop {
            let Some(ch) = self.src[self.pos..].chars().next() else {
                return Err(self.error("unterminated string"));
            };
            self.pos += ch.len_utf8();
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let Some(escape) = self.src[self.pos..].chars().next() else {
                        return Err(self.error("unterminated string"));
                    };
                    self.pos += escape.len_utf8();
                    match escape {
                        '"' | '\\' | '/' => out.push(escape),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => out.push(self.unicode_escape()?),
                        _ => return Err(self.error("invalid escape")),
                    }
                }
                other => out.push(other),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .src
            .get(self.pos..self.pos + 4)
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .ok_or_else(|| self.error("invalid \\u escape"))?;
        self.pos += 4;
        Ok(digits)
    }

    /// The character a `\u` escape names, after the `u`; a surrogate pair is two escapes.
    fn unicode_escape(&mut self) -> Result<char, String> {
        let unit = self.hex4()?;
        let code = if (0xD800..0xDC00).contains(&unit) {
            if !self.src[self.pos..].starts_with("\\u") {
                return Err(self.error("lone surrogate in \\u escape"));
            }
            self.pos += 2;
            let low = self.hex4()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(self.error("invalid surrogate pair"));
            }
            0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00)
        } else {
            unit
        };
        char::from_u32(code).ok_or_else(|| self.error("invalid \\u escape"))
    }

    fn number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E'))
        {
            self.pos += 1;
        }
        match serde_json::from_str::<Value>(&self.src[start..self.pos]) {
            Ok(number @ Value::Number(_)) => Ok(number),
            _ => {
                self.pos = start;
                Err(self.error("invalid number"))
            }
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Result<Value, String> {
        if self.src[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.error("expected a value"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Result<Document, String> {
        super::parse(src, true)
    }

    #[test]
    fn spans_are_one_based_line_and_column_of_each_value() {
        let doc = parse("{\n  \"a\": 1,\n  \"b\": [true, \"x\"]\n}\n").expect("parses");
        let at = |pointer: &str| {
            let span = doc.span_of(pointer).expect("span");
            (span.start_line, span.start_col, span.end_line, span.end_col)
        };
        assert_eq!(at("/a"), (2, 8, 2, 9));
        assert_eq!(at("/b/0"), (3, 9, 3, 13));
        assert_eq!(at("/b/1"), (3, 15, 3, 18));
        assert_eq!(at(""), (1, 1, 4, 2));
    }

    #[test]
    fn comments_and_trailing_commas_are_accepted() {
        let doc = parse("// lead\n{ /* c */ \"a\": [1, 2,], }\n").expect("parses");
        assert_eq!(doc.value, serde_json::json!({"a": [1, 2]}));
    }

    #[test]
    fn escapes_and_surrogate_pairs_decode() {
        let doc = parse(r#"{"k": "a\né😀"}"#).expect("parses");
        assert_eq!(doc.value["k"], "a\n\u{e9}\u{1F600}");
    }

    #[test]
    fn syntax_errors_carry_a_position() {
        assert!(parse("{\"a\": }").unwrap_err().contains("1:7"));
        assert!(parse("{\"a\": 1} x").is_err());
        assert!(parse("[01]").is_err());
        assert!(parse("\"abc").is_err());
    }

    #[test]
    fn deeply_nested_input_is_refused() {
        let deep = "[".repeat(MAX_DEPTH + 8);
        assert!(parse(&deep).unwrap_err().contains("too deep"));
    }
}
