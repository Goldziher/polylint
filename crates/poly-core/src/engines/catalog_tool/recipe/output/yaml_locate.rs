//! Mapping a resource and a JSON pointer inside it to a line of a multi-document
//! YAML manifest, for reports (kubeconform) that name neither a line nor a column.
//!
//! The reader is indentation-based and block-style only: it finds the document
//! whose `kind` and `metadata.name` match, then follows the pointer through
//! mapping keys and sequence indices. Where the manifest uses a form it cannot
//! follow (flow style, anchors), the deepest line it did reach is reported, and
//! the document's first line when nothing was reached.

/// The resource a finding is about and where in it the problem sits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::engines::catalog_tool) struct Locus {
    pub(in crate::engines::catalog_tool) kind: String,
    pub(in crate::engines::catalog_tool) name: String,
    /// A JSON pointer (`/spec/containers/0/image`); empty for the whole resource.
    pub(in crate::engines::catalog_tool) pointer: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A `- ` sequence marker.
    Dash,
    /// Anything else on the line after the markers.
    Text,
}

/// One syntactic unit of a line: a sequence marker or the text after the
/// markers. `- name: app` is a `Dash` at indent 2 and a `Text` at indent 4.
struct Entry<'a> {
    /// 1-based line number.
    line: u32,
    indent: usize,
    kind: Kind,
    text: &'a str,
}

/// The 1-based line `locus` points at within `content`, or `None` when no
/// document of the manifest is that resource.
pub(in crate::engines::catalog_tool) fn locate(content: &str, locus: &Locus) -> Option<u32> {
    let documents = documents(content);
    let document = documents.iter().find(|doc| matches(doc, locus))?;
    let first = document.first()?.line;
    let segments = pointer_segments(&locus.pointer);
    Some(follow(document, &segments).unwrap_or(first))
}

fn documents(content: &str) -> Vec<Vec<Entry<'_>>> {
    let mut documents = vec![Vec::new()];
    for (index, raw) in content.lines().enumerate() {
        let line = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if raw.starts_with("---") || raw.trim_end() == "..." {
            documents.push(Vec::new());
            let rest = raw.trim_start_matches('-').trim();
            if raw.starts_with("--- ") && !rest.is_empty() && !rest.starts_with('#') {
                push_entries(documents.last_mut().expect("a document"), line, 0, rest);
            }
            continue;
        }
        let trimmed = raw.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = raw.len() - trimmed.len();
        push_entries(documents.last_mut().expect("a document"), line, indent, trimmed);
    }
    documents.retain(|doc| !doc.is_empty());
    documents
}

fn push_entries<'a>(out: &mut Vec<Entry<'a>>, line: u32, mut indent: usize, mut text: &'a str) {
    while text == "-" || text.starts_with("- ") {
        out.push(Entry {
            line,
            indent,
            kind: Kind::Dash,
            text: "",
        });
        let rest = text[1..].trim_start();
        indent += text.len() - rest.len();
        text = rest;
    }
    if !text.is_empty() {
        out.push(Entry {
            line,
            indent,
            kind: Kind::Text,
            text,
        });
    }
}

fn pointer_segments(pointer: &str) -> Vec<String> {
    pointer
        .split('/')
        .skip(1)
        .map(|part| part.replace("~1", "/").replace("~0", "~"))
        .collect()
}

/// The key of a `key: value` line, unquoted.
fn key_of(text: &str) -> Option<&str> {
    let colon = if let Some(quote) = text.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        let close = text[1..].find(quote)? + 1;
        close + text[close..].find(':')?
    } else {
        let mut at = None;
        for (position, _) in text.match_indices(':') {
            if text[position + 1..].is_empty() || text[position + 1..].starts_with([' ', '\t']) {
                at = Some(position);
                break;
            }
        }
        at?
    };
    Some(text[..colon].trim().trim_matches(['"', '\'']))
}

fn value_of(text: &str) -> String {
    let after = text.split_once(':').map_or("", |(_, value)| value);
    let value = after.split(" #").next().unwrap_or(after).trim();
    value.trim_matches(['"', '\'']).to_owned()
}

/// The entry index range that is the content nested under entry `at`.
fn children(entries: &[Entry<'_>], at: usize, end: usize) -> (usize, usize) {
    let indent = entries[at].indent;
    let stop = (at + 1..end)
        .find(|&i| {
            let entry = &entries[i];
            entry.indent < indent || (entry.indent == indent && entry.kind != Kind::Dash)
        })
        .unwrap_or(end);
    (at + 1, stop)
}

/// The entry for `segment` in the block `lo..hi`, whose entries share the
/// indent of the first one.
fn step(entries: &[Entry<'_>], lo: usize, hi: usize, segment: &str) -> Option<usize> {
    let block = entries.get(lo).filter(|_| lo < hi)?.indent;
    if let Ok(wanted) = segment.parse::<usize>() {
        let dash = (lo..hi)
            .filter(|&i| entries[i].indent == block && entries[i].kind == Kind::Dash)
            .nth(wanted);
        if dash.is_some() {
            return dash;
        }
    }
    (lo..hi).find(|&i| {
        let entry = &entries[i];
        entry.indent == block && entry.kind == Kind::Text && key_of(entry.text) == Some(segment)
    })
}

/// Follow `segments` from the start of `entries`; the line of the deepest entry
/// reached, or `None` when not even the first segment resolved.
fn follow(entries: &[Entry<'_>], segments: &[String]) -> Option<u32> {
    let (mut lo, mut hi) = (0, entries.len());
    let mut reached = None;
    for segment in segments {
        let Some(at) = step(entries, lo, hi, segment) else {
            break;
        };
        reached = Some(entries[at].line);
        (lo, hi) = children(entries, at, hi);
    }
    reached
}

/// The entry index of the scalar at `path`, followed from the top of `entries`.
fn scalar_at(entries: &[Entry<'_>], path: &[&str]) -> Option<String> {
    let (mut lo, mut hi) = (0, entries.len());
    let mut found = None;
    for segment in path {
        let at = step(entries, lo, hi, segment)?;
        found = Some(at);
        (lo, hi) = children(entries, at, hi);
    }
    found.map(|at| value_of(entries[at].text))
}

fn matches(entries: &[Entry<'_>], locus: &Locus) -> bool {
    if scalar_at(entries, &["kind"]).as_deref() != Some(locus.kind.as_str()) {
        return false;
    }
    locus.name.is_empty() || scalar_at(entries, &["metadata", "name"]).as_deref() == Some(locus.name.as_str())
}
