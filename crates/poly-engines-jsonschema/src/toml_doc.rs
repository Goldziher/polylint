//! A TOML reader that records the source span of every value, over the spanned `toml` parser.
//!
//! `toml::de::DeTable` keeps a byte range for each key and value. A table is placed at its key,
//! since a table value's own range is a header or body rather than the name a reader looks for.

use std::collections::HashMap;
use std::ops::Range;

use poly_engine_api::engine::Span;
use poly_engine_api::line_index::LineIndex;
use serde_json::{Map, Number, Value};
use toml::de::{DeInteger, DeTable, DeValue};

use crate::doc::{Document, child_if, element_if, span_between};

/// Parses `src` into its value and spans.
///
/// # Errors
///
/// The parser's message, with its position, when `src` is not valid TOML.
pub(crate) fn parse(src: &str, record: bool) -> Result<Document, String> {
    let root = DeTable::parse(src).map_err(|error| format!("invalid TOML: {error}"))?;
    let lines = LineIndex::new(src);
    let mut spans = record.then(HashMap::new);
    let value = table(root.get_ref(), "", &lines, &mut spans);
    Ok(match spans {
        Some(spans) => Document::new(value, spans),
        None => Document::unrecorded(value),
    })
}

type Spans = Option<HashMap<String, Span>>;

fn record(spans: &mut Spans, lines: &LineIndex<'_>, pointer: &str, range: Range<usize>) {
    if let Some(spans) = spans {
        spans.insert(pointer.to_owned(), span_between(lines, range.start, range.end));
    }
}

fn table(entries: &DeTable<'_>, pointer: &str, lines: &LineIndex<'_>, spans: &mut Spans) -> Value {
    let mut members = Map::new();
    for (key, value) in entries.iter() {
        let name: &str = key.get_ref();
        let member = child_if(spans.is_some(), pointer, name);
        let range = if matches!(value.get_ref(), DeValue::Table(_)) {
            key.span()
        } else {
            value.span()
        };
        record(spans, lines, &member, range);
        members.insert(name.to_owned(), node(value.get_ref(), &member, lines, spans));
    }
    Value::Object(members)
}

fn node(value: &DeValue<'_>, pointer: &str, lines: &LineIndex<'_>, spans: &mut Spans) -> Value {
    match value {
        DeValue::String(text) => Value::String(text.to_string()),
        DeValue::Integer(integer) => integer_value(integer),
        DeValue::Float(float) => {
            let text = float.as_str().replace('_', "");
            text.parse::<f64>()
                .ok()
                .and_then(Number::from_f64)
                .map_or_else(|| Value::String(text), Value::Number)
        }
        DeValue::Boolean(flag) => Value::Bool(*flag),
        DeValue::Datetime(moment) => Value::String(moment.to_string()),
        DeValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for (at, item) in items.iter().enumerate() {
                let member = element_if(spans.is_some(), pointer, at);
                record(spans, lines, &member, item.span());
                out.push(node(item.get_ref(), &member, lines, spans));
            }
            Value::Array(out)
        }
        DeValue::Table(entries) => table(entries, pointer, lines, spans),
    }
}

fn integer_value(integer: &DeInteger<'_>) -> Value {
    let text = integer.as_str().replace('_', "");
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(&text)),
    };
    let digits = ["0x", "0o", "0b"]
        .iter()
        .find_map(|prefix| digits.strip_prefix(prefix))
        .unwrap_or(digits);
    let signed = if negative {
        format!("-{digits}")
    } else {
        digits.to_owned()
    };
    match i64::from_str_radix(&signed, integer.radix()) {
        Ok(number) => Value::from(number),
        Err(_) => signed
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map_or_else(|| Value::String(text), Value::Number),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Result<Document, String> {
        super::parse(src, true)
    }

    #[test]
    fn values_and_positions_come_from_the_source() {
        let doc = parse("name = \"app\"\nport = \"80\"\n[server]\nthreads = 4\n").expect("parses");
        assert_eq!(
            doc.value,
            serde_json::json!({"name": "app", "port": "80", "server": {"threads": 4}})
        );
        let at = |pointer: &str| {
            let span = doc.span_of(pointer).expect("span");
            (span.start_line, span.start_col)
        };
        assert_eq!(at("/port"), (2, 8));
        assert_eq!(at("/server/threads"), (4, 11));
        assert_eq!(at("/server"), (3, 2));
    }

    #[test]
    fn integers_floats_dates_and_arrays_convert() {
        let doc = parse("a = 0x10\nb = 1_000\nc = 1.5\nd = 2024-01-02\ne = [1, 2]\n").expect("parses");
        assert_eq!(
            doc.value,
            serde_json::json!({"a": 16, "b": 1000, "c": 1.5, "d": "2024-01-02", "e": [1, 2]})
        );
    }

    #[test]
    fn syntax_errors_are_reported() {
        assert!(parse("a = \n").is_err());
    }
}
