//! A parsed document: the value that is validated and the source span of every node in it.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use poly_engine_api::engine::Span;
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;
use serde_json::Value;

/// Nesting deeper than this is refused rather than recursed into.
pub(crate) const MAX_DEPTH: usize = 128;

/// Where to re-read a document from when a finding needs its spans.
#[derive(Debug)]
struct Reparse {
    language: Language,
    src: Arc<str>,
    index: usize,
}

/// One document of a file: its value and, once a finding asks, where each node of it sits in the
/// source. A valid document never needs positions, so they are recorded by a second parse only for
/// a document that has findings.
#[derive(Debug)]
pub(crate) struct Document {
    pub(crate) value: Value,
    spans: OnceLock<HashMap<String, Span>>,
    source: Option<Reparse>,
}

impl Document {
    /// A document whose spans were recorded while it was parsed.
    pub(crate) fn new(value: Value, spans: HashMap<String, Span>) -> Self {
        Self {
            value,
            spans: OnceLock::from(spans),
            source: None,
        }
    }

    /// A document parsed without spans.
    pub(crate) fn unrecorded(value: Value) -> Self {
        Self {
            value,
            spans: OnceLock::new(),
            source: None,
        }
    }

    fn spans(&self) -> &HashMap<String, Span> {
        self.spans.get_or_init(|| {
            self.source
                .as_ref()
                .and_then(|source| {
                    parse_documents(&source.language, &source.src, true)
                        .ok()?
                        .into_iter()
                        .nth(source.index)
                })
                .and_then(|document| document.spans.into_inner())
                .unwrap_or_default()
        })
    }

    /// The span of the node a JSON Pointer names, or of its nearest ancestor that has one.
    pub(crate) fn span_of(&self, pointer: &str) -> Option<Span> {
        let spans = self.spans();
        let mut pointer = pointer;
        loop {
            if let Some(span) = spans.get(pointer) {
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

/// [`child`] when `record` is set, else an empty pointer nobody reads.
pub(crate) fn child_if(record: bool, parent: &str, key: &str) -> String {
    if record { child(parent, key) } else { String::new() }
}

/// [`element`] when `record` is set, else an empty pointer nobody reads.
pub(crate) fn element_if(record: bool, parent: &str, at: usize) -> String {
    if record { element(parent, at) } else { String::new() }
}

/// The pointer to element `at` of the array at `parent`.
pub(crate) fn element(parent: &str, at: usize) -> String {
    format!("{parent}/{at}")
}

/// The span covering `start..end` (byte offsets) of the source `lines` indexes.
pub(crate) fn span_between(lines: &LineIndex<'_>, start: usize, end: usize) -> Span {
    lines.span(start, end)
}

/// Parses `src` as the documents of `language` without recording spans; a document resolves them
/// on first use by parsing `src` again.
///
/// # Errors
///
/// The syntax error, positioned, when `src` is not valid in its language.
pub(crate) fn parse_lazy(language: &Language, src: &Arc<str>) -> Result<Vec<Document>, String> {
    let mut documents = parse_documents(language, src, false)?;
    for (index, document) in documents.iter_mut().enumerate() {
        document.source = Some(Reparse {
            language: language.clone(),
            src: Arc::clone(src),
            index,
        });
    }
    Ok(documents)
}

/// Parses `src` as the documents of `language`: one for JSON, JSONC and TOML, one per `---`
/// document for YAML. `record` says whether each node's span is kept.
fn parse_documents(language: &Language, src: &str, record: bool) -> Result<Vec<Document>, String> {
    match language {
        Language::Json | Language::Jsonc => crate::json::parse(src, record).map(|doc| vec![doc]),
        Language::Yaml => crate::yaml::parse(src, record),
        Language::Toml => crate::toml_doc::parse(src, record).map(|doc| vec![doc]),
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

    /// The spans the eager parse recorded for every node, the behaviour before spans became lazy.
    fn reference_spans(language: &Language, src: &str) -> Vec<HashMap<String, Span>> {
        parse_documents(language, src, true)
            .expect("parses")
            .into_iter()
            .map(|document| document.spans.into_inner().expect("recorded"))
            .collect()
    }

    fn assert_lazy_matches_eager(language: Language, src: &str) {
        let reference = reference_spans(&language, src);
        assert!(reference.iter().any(|spans| spans.len() > 1), "fixture records spans");
        let text: Arc<str> = src.into();
        let lazy = parse_lazy(&language, &text).expect("parses");
        assert_eq!(lazy.len(), reference.len());
        for (document, spans) in lazy.iter().zip(&reference) {
            assert!(
                document.spans.get().is_none(),
                "nothing is recorded before it is asked for"
            );
            for (pointer, span) in spans {
                assert_eq!(document.span_of(pointer), Some(*span), "pointer `{pointer}`");
            }
            assert_eq!(document.spans(), spans);
        }
    }

    #[test]
    fn lazy_spans_equal_the_eager_map_for_json() {
        assert_lazy_matches_eager(
            Language::Json,
            "{\n  \"a\": 1,\n  \"b/c\": [true, {\"d~\": null}],\n  \"e\": {\"f\": \"x\"}\n}\n",
        );
    }

    #[test]
    fn lazy_spans_equal_the_eager_map_for_yaml() {
        assert_lazy_matches_eager(
            Language::Yaml,
            "base: &b {k: 1}\nsvc:\n  <<: *b\n  list: [1, two]\n---\nz: {y: [a, b]}\n",
        );
    }

    #[test]
    fn lazy_spans_equal_the_eager_map_for_toml() {
        assert_lazy_matches_eager(
            Language::Toml,
            "name = \"app\"\nports = [80, 443]\n[server]\nthreads = 4\n[server.tls]\non = true\n",
        );
    }
}
