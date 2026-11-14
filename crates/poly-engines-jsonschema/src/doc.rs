//! A parsed document: the value that is validated and the source span of every node in it.

use std::collections::HashMap;

use poly_engine_api::engine::Span;
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;
use serde_json::Value;

/// Nesting deeper than this is refused rather than recursed into.
pub(crate) const MAX_DEPTH: usize = 128;

/// One document of a file: its value and where each node of it sits in the source.
#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) value: Value,
    spans: HashMap<String, Span>,
}

impl Document {
    pub(crate) fn new(value: Value, spans: HashMap<String, Span>) -> Self {
        Self { value, spans }
    }

    /// The span of the node a JSON Pointer names, or of its nearest ancestor that has one.
    pub(crate) fn span_of(&self, pointer: &str) -> Option<Span> {
        let mut pointer = pointer;
        loop {
            if let Some(span) = self.spans.get(pointer) {
                return Some(*span);
            }
            pointer = &pointer[..pointer.rfind('/')?];
        }
    }
}

/// The pointer to member `key` of the object at `parent`.
pub(crate) fn child(parent: &str, key: &str) -> String {
    format!("{parent}/{}", key.replace('~', "~0").replace('/', "~1"))
}

/// The pointer to element `at` of the array at `parent`.
pub(crate) fn element(parent: &str, at: usize) -> String {
    format!("{parent}/{at}")
}

/// The span covering `start..end` (byte offsets) of the source `lines` indexes.
pub(crate) fn span_between(lines: &LineIndex<'_>, start: usize, end: usize) -> Span {
    let (start_line, start_col) = lines.line_col(start);
    let (end_line, end_col) = lines.line_col(end);
    Span {
        start_line,
        start_col,
        end_line,
        end_col,
    }
}

/// Parses `src` as the documents of `language`: one for JSON, JSONC and TOML, one per `---`
/// document for YAML.
///
/// # Errors
///
/// The syntax error, positioned, when `src` is not valid in its language.
pub(crate) fn parse(language: &Language, src: &str) -> Result<Vec<Document>, String> {
    match language {
        Language::Json | Language::Jsonc => crate::json::parse(src).map(|doc| vec![doc]),
        Language::Yaml => crate::yaml::parse(src),
        Language::Toml => crate::toml_doc::parse(src).map(|doc| vec![doc]),
        other => Err(format!("{} is not a language the schema engine reads", other.id())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_escape_tilde_and_slash() {
        assert_eq!(child("", "a/b"), "/a~1b");
        assert_eq!(child("/x", "m~n"), "/x/m~0n");
        assert_eq!(element("/xs", 3), "/xs/3");
    }

    #[test]
    fn a_span_falls_back_to_the_nearest_ancestor() {
        let span = Span {
            start_line: 2,
            start_col: 3,
            end_line: 2,
            end_col: 9,
        };
        let doc = Document::new(Value::Null, HashMap::from([("/a".to_owned(), span)]));
        assert_eq!(doc.span_of("/a/b/0"), Some(span));
        assert_eq!(doc.span_of("/z"), None);
        assert_eq!(doc.span_of(""), None);
    }
}
