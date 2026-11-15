//! A YAML reader that records the source span of every node, over the `granit-parser` event stream.
//!
//! The same parser backs `serde-saphyr`, which poly already links, and its events carry byte
//! ranges, so each node's position comes from the parser rather than from a second scan of the
//! text. Plain scalars resolve as the YAML 1.2 core schema does (null, bool, int, float, else
//! string); quoted and tagged scalars stay strings. A multi-document stream yields one
//! [`Document`] per document.

use std::collections::HashMap;
use std::ops::Range;

use granit_parser::{Event, Parser, ScalarStyle};
use poly_engine_api::engine::Span;
use poly_engine_api::line_index::LineIndex;
use serde_json::{Map, Number, Value};

use crate::doc::{Document, MAX_DEPTH, child, element, span_between};

/// Where an event sits in the source.
#[derive(Clone)]
struct Loc {
    bytes: Option<Range<usize>>,
    start: (usize, usize),
}

/// The parts of an event the walker needs, owned so the event buffer is not borrowed while walking.
enum Kind {
    Scalar { text: String, plain: bool, anchor: usize },
    SeqStart(usize),
    MapStart(usize),
    Alias(usize),
    SeqEnd,
    MapEnd,
    DocStart,
    DocEnd,
}

struct Walker<'a> {
    lines: LineIndex<'a>,
    events: Vec<Option<(Kind, Loc)>>,
    at: usize,
    anchors: HashMap<usize, Value>,
    spans: HashMap<String, Span>,
}

/// Parses every document of `src`.
///
/// # Errors
///
/// The parser's message, with its position, when `src` is not valid YAML.
pub(crate) fn parse(src: &str) -> Result<Vec<Document>, String> {
    let mut events = Vec::new();
    for event in Parser::new_from_str(src) {
        let (event, span) = event.map_err(|error| format!("invalid YAML: {error}"))?;
        let kind = match &event {
            Event::Scalar(text, style, anchor, tag) => Kind::Scalar {
                text: text.to_string(),
                plain: matches!(style, ScalarStyle::Plain) && tag.is_none(),
                anchor: *anchor,
            },
            Event::SequenceStart(_, anchor, _) => Kind::SeqStart(*anchor),
            Event::MappingStart(_, anchor, _) => Kind::MapStart(*anchor),
            Event::Alias(anchor) => Kind::Alias(*anchor),
            Event::SequenceEnd => Kind::SeqEnd,
            Event::MappingEnd => Kind::MapEnd,
            Event::DocumentStart(..) => Kind::DocStart,
            Event::DocumentEnd => Kind::DocEnd,
            _ => continue,
        };
        let loc = Loc {
            bytes: span.byte_range(),
            start: (span.start.line(), span.start.col() + 1),
        };
        events.push(Some((kind, loc)));
    }
    let mut walker = Walker {
        lines: LineIndex::new(src),
        events,
        at: 0,
        anchors: HashMap::new(),
        spans: HashMap::new(),
    };
    let mut documents = Vec::new();
    while let Some((kind, _)) = walker.pull() {
        if !matches!(kind, Kind::DocStart) {
            continue;
        }
        walker.anchors.clear();
        walker.spans.clear();
        let (root, loc) = walker
            .pull()
            .ok_or_else(|| "invalid YAML: truncated document".to_owned())?;
        let value = walker.node(root, &loc, "", 0)?;
        while let Some((kind, _)) = walker.pull() {
            if matches!(kind, Kind::DocEnd) {
                break;
            }
        }
        documents.push(Document::new(value, std::mem::take(&mut walker.spans)));
    }
    Ok(documents)
}

/// The value of the first document of `src`, for a schema written in YAML.
///
/// # Errors
///
/// The parser's message when `src` is not valid YAML, or a note that it holds no document.
pub(crate) fn parse_value(src: &str) -> Result<Value, String> {
    parse(src)?
        .into_iter()
        .next()
        .map(|document| document.value)
        .ok_or_else(|| "invalid YAML: no document".to_owned())
}

impl Walker<'_> {
    fn pull(&mut self) -> Option<(Kind, Loc)> {
        let event = self.events.get_mut(self.at)?.take();
        self.at += 1;
        event
    }

    fn record(&mut self, pointer: &str, loc: &Loc) {
        let span = match &loc.bytes {
            Some(range) => span_between(&self.lines, range.start, range.end),
            None => {
                let line = u32::try_from(loc.start.0).unwrap_or(1);
                let col = u32::try_from(loc.start.1).unwrap_or(1);
                Span {
                    start_line: line,
                    start_col: col,
                    end_line: line,
                    end_col: col,
                }
            }
        };
        self.spans.insert(pointer.to_owned(), span);
    }

    fn node(&mut self, kind: Kind, loc: &Loc, pointer: &str, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("invalid YAML: nesting is too deep".to_owned());
        }
        let truncated = || "invalid YAML: truncated collection".to_owned();
        let (value, anchor) = match kind {
            Kind::Scalar { text, plain, anchor } => (scalar(&text, plain), anchor),
            Kind::Alias(anchor) => {
                let value = self.anchors.get(&anchor).cloned().unwrap_or(Value::Null);
                self.record(pointer, loc);
                return Ok(value);
            }
            Kind::SeqStart(anchor) => {
                let mut items = Vec::new();
                loop {
                    let (kind, item_loc) = self.pull().ok_or_else(truncated)?;
                    if matches!(kind, Kind::SeqEnd) {
                        break;
                    }
                    let value = self.node(kind, &item_loc, &element(pointer, items.len()), depth + 1)?;
                    items.push(value);
                }
                (Value::Array(items), anchor)
            }
            Kind::MapStart(anchor) => {
                let mut members = Map::new();
                let mut merged: Vec<(Map<String, Value>, Loc)> = Vec::new();
                loop {
                    let (kind, key_loc) = self.pull().ok_or_else(truncated)?;
                    let merge_key = matches!(&kind, Kind::Scalar { text, plain: true, .. } if text == "<<");
                    let key = match kind {
                        Kind::MapEnd => break,
                        Kind::Scalar { text, .. } => text,
                        other => {
                            self.node(other, &key_loc, "/~complex-key", depth + 1)?;
                            "<complex key>".to_owned()
                        }
                    };
                    let (kind, value_loc) = self.pull().ok_or_else(truncated)?;
                    if merge_key {
                        let value = self.node(kind, &value_loc, "/~merge", depth + 1)?;
                        match merge_sources(&value) {
                            Some(sources) => {
                                merged.extend(sources.into_iter().map(|source| (source, key_loc.clone())));
                                continue;
                            }
                            None => {
                                self.record(&child(pointer, &key), &value_loc);
                                members.insert(key, value);
                                continue;
                            }
                        }
                    }
                    let value = self.node(kind, &value_loc, &child(pointer, &key), depth + 1)?;
                    members.insert(key, value);
                }
                for (source, merge_loc) in merged {
                    for (key, value) in source {
                        if !members.contains_key(&key) {
                            self.record(&child(pointer, &key), &merge_loc);
                            members.insert(key, value);
                        }
                    }
                }
                (Value::Object(members), anchor)
            }
            Kind::SeqEnd | Kind::MapEnd | Kind::DocStart | Kind::DocEnd => {
                return Err("invalid YAML: unexpected structure".to_owned());
            }
        };
        if anchor != 0 {
            self.anchors.insert(anchor, value.clone());
        }
        self.record(pointer, loc);
        Ok(value)
    }
}

/// The mappings a `<<` merge key pulls in: the mapping itself, or each mapping of a sequence of
/// them. `None` when the value is neither, so the key stays an ordinary one.
fn merge_sources(value: &Value) -> Option<Vec<Map<String, Value>>> {
    match value {
        Value::Object(map) => Some(vec![map.clone()]),
        Value::Array(items) => items.iter().map(|item| item.as_object().cloned()).collect(),
        _ => None,
    }
}

/// A scalar's value: plain scalars resolve by the YAML 1.2 core schema, anything else is a string.
fn scalar(text: &str, plain: bool) -> Value {
    if !plain {
        return Value::String(text.to_owned());
    }
    match text {
        "" | "~" | "null" | "Null" | "NULL" => Value::Null,
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        _ => number(text).unwrap_or_else(|| Value::String(text.to_owned())),
    }
}

fn number(text: &str) -> Option<Value> {
    for (prefix, radix) in [("0x", 16), ("0o", 8)] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return i64::from_str_radix(digits, radix).ok().map(Value::from);
        }
    }
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let numeric = body.starts_with(|ch: char| ch.is_ascii_digit() || ch == '.')
        && body.chars().any(|ch| ch.is_ascii_digit())
        && body
            .chars()
            .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | 'e' | 'E' | '+' | '-'));
    if !numeric {
        return None;
    }
    if body.chars().all(|ch| ch.is_ascii_digit()) {
        let integer = text
            .parse::<i64>()
            .ok()
            .map(Value::from)
            .or_else(|| text.parse::<u64>().ok().map(Value::from));
        if integer.is_some() {
            return integer;
        }
    }
    text.parse::<f64>().ok().and_then(Number::from_f64).map(Value::Number)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(doc: &Document, pointer: &str) -> (u32, u32) {
        let span = doc.span_of(pointer).expect("span");
        (span.start_line, span.start_col)
    }

    #[test]
    fn block_mappings_resolve_types_and_positions() {
        let docs = parse("name: app\nport: \"80\"\nreplicas: 3\nitems:\n  - a\n  - 2.5\n").expect("parses");
        assert_eq!(docs.len(), 1);
        let doc = &docs[0];
        assert_eq!(
            doc.value,
            serde_json::json!({"name": "app", "port": "80", "replicas": 3, "items": ["a", 2.5]})
        );
        assert_eq!(at(doc, "/port"), (2, 7));
        assert_eq!(at(doc, "/replicas"), (3, 11));
        assert_eq!(at(doc, "/items/1"), (6, 5));
    }

    #[test]
    fn each_document_of_a_stream_is_its_own() {
        let docs = parse("a: 1\n---\nb: 2\n").expect("parses");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[1].value, serde_json::json!({"b": 2}));
        assert_eq!(at(&docs[1], "/b"), (3, 4));
    }

    #[test]
    fn anchors_and_aliases_resolve_to_the_anchored_value() {
        let docs = parse("base: &b {x: 1}\ncopy: *b\n").expect("parses");
        assert_eq!(docs[0].value["copy"], serde_json::json!({"x": 1}));
    }

    fn merged(src: &str) -> Value {
        parse(src).expect("parses").remove(0).value
    }

    #[test]
    fn a_merge_key_pulls_in_an_aliased_mapping() {
        let value = merged("x: &d {name: a, n: 1}\nsvc: {<<: *d, n: 2}\n");
        assert_eq!(value["svc"], serde_json::json!({"name": "a", "n": 2}));
    }

    #[test]
    fn a_merge_sequence_prefers_earlier_mappings() {
        let value = merged("a: &a {k: 1}\nb: &b {k: 2, m: 3}\nsvc:\n  <<: [*a, *b]\n");
        assert_eq!(value["svc"], serde_json::json!({"k": 1, "m": 3}));
    }

    #[test]
    fn an_explicit_key_wins_over_a_merged_one_wherever_it_sits() {
        let value = merged("a: &a {k: 1}\nsvc:\n  k: 9\n  <<: *a\n");
        assert_eq!(value["svc"], serde_json::json!({"k": 9}));
    }

    #[test]
    fn a_merged_mapping_may_itself_merge() {
        let value = merged("a: &a {x: 1}\nb: &b\n  <<: *a\n  y: 2\nc:\n  <<: *b\n");
        assert_eq!(value["c"], serde_json::json!({"x": 1, "y": 2}));
    }

    #[test]
    fn a_quoted_or_non_mapping_merge_key_stays_literal() {
        let value = merged("a: &a {k: 1}\nq: {\"<<\": *a}\ns: {<<: 5}\n");
        assert_eq!(value["q"], serde_json::json!({"<<": {"k": 1}}));
        assert_eq!(value["s"], serde_json::json!({"<<": 5}));
    }

    #[test]
    fn merged_keys_point_at_the_merge_key() {
        let docs = parse("a: &a {k: 1}\nsvc:\n  <<: *a\n").expect("parses");
        assert_eq!(at(&docs[0], "/svc/k"), (3, 3));
    }

    #[test]
    fn plain_scalars_follow_the_core_schema() {
        assert_eq!(scalar("~", true), Value::Null);
        assert_eq!(scalar("True", true), Value::Bool(true));
        assert_eq!(scalar("0x1F", true), Value::from(31));
        assert_eq!(scalar("2024-01-01", true), Value::String("2024-01-01".into()));
        assert_eq!(scalar("1.2.3", true), Value::String("1.2.3".into()));
        assert_eq!(scalar("1e3", true), Value::from(1000.0));
        assert_eq!(scalar("true", false), Value::String("true".into()));
    }

    #[test]
    fn syntax_errors_are_reported() {
        assert!(parse("a: [1, 2\n").is_err());
    }
}
