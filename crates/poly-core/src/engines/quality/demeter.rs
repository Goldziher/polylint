//! `law-of-demeter` (opt-in): a chain of member/attribute accesses longer
//! than the configured depth (default 3), e.g. `a.b.c.d.e`.
//!
//! Best-effort and grammar-generic, like [`super::magic_number`]: rather than
//! a per-language table, this matches any node whose kind name contains
//! `member_expression`, `field_expression`, `attribute`, or
//! `selector_expression` — the common tree-sitter names for `a.b` access
//! across the languages this engine covers — and counts how many such nodes
//! chain together without an intervening call. A method-call chain
//! (`a.b().c().d()`, the fluent-builder pattern) is **not** flagged: each
//! call is a deliberate step the author chose to chain, which is a different
//! smell than reaching through unrelated objects' internals.
//!
//! Only the **outermost** node of a chain is reported (an inner
//! `a.b.c` within a longer `a.b.c.d.e` is not separately flagged), so one
//! overlong chain produces exactly one finding.

use tree_sitter::Node;

use super::kinds::KindTable;
use super::metrics::span_of;
use super::walk::walk;
use crate::engine::{Diagnostic, Severity};
use crate::engines::line_index::LineIndex;

const CHAIN_KIND_MARKERS: &[&str] = &[
    "member_expression",
    "field_expression",
    "attribute",
    "selector_expression",
];

/// Walk `root` and report every access chain deeper than `max_depth`.
///
/// Reported at the outermost chain node only: a chain-kind node whose parent is also chain-kind is
/// a link in a longer chain. Call arguments and index expressions are still searched for chains of
/// their own, since their parent is not a chain-kind node.
pub fn scan(root: Node, source: &str, max_depth: usize) -> Vec<Diagnostic> {
    scan_with(
        &KindTable::build("", &root.language()),
        root,
        &LineIndex::new(source),
        max_depth,
    )
}

/// [`scan`] against a prebuilt per-grammar [`KindTable`].
pub fn scan_with(table: &KindTable, root: Node, index: &LineIndex<'_>, max_depth: usize) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    walk(
        root,
        |node, ancestors| {
            if table.info(node.kind_id()).chain
                && !ancestors
                    .last()
                    .is_some_and(|parent| table.info(parent.kind_id()).chain)
            {
                let depth = chain_depth(table, node);
                if depth > max_depth {
                    out.push(diagnostic(node, index, depth));
                }
            }
        },
        |_, _| {},
    );
    out
}

pub(super) fn is_chain_kind(kind: &str) -> bool {
    let lower = kind.to_ascii_lowercase();
    CHAIN_KIND_MARKERS.iter().any(|marker| lower.contains(marker))
}

/// Count how many chain-kind nodes are nested along the "object" side of
/// `node`, i.e. its first named child, its first named child's first named
/// child, and so on.
fn chain_depth(table: &KindTable, node: Node) -> usize {
    let mut depth = 1;
    let mut current = node;
    while let Some(object) = current.named_child(0) {
        if table.info(object.kind_id()).chain {
            depth += 1;
            current = object;
        } else {
            break;
        }
    }
    depth
}

fn diagnostic(node: Node, index: &LineIndex<'_>, depth: usize) -> Diagnostic {
    Diagnostic {
        engine: "quality".to_owned(),
        code: Some("law-of-demeter".to_owned()),
        severity: Severity::Warning,
        title: format!("access chain is {depth} levels deep"),
        description: Some("reach through fewer objects' internals, or add a helper method".to_owned()),
        span: Some(span_of(index, node.start_byte(), node.end_byte())),
        url: None,
        fix: Vec::new(),
        metadata: std::collections::BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;
    use tree_sitter_language_pack::get_language;

    use super::*;

    fn parse(grammar: &str, source: &str) -> tree_sitter::Tree {
        let language = get_language(grammar).expect("grammar available");
        let mut parser = Parser::new();
        parser.set_language(&language).expect("set_language");
        parser.parse(source, None).expect("parse")
    }

    #[test]
    fn flags_a_deep_attribute_chain() {
        let source = "a.b.c.d.e\n";
        let tree = parse("python", source);
        let findings = scan(tree.root_node(), source, 3);
        assert_eq!(findings.len(), 1, "expected exactly one finding for the whole chain");
        assert_eq!(findings[0].code.as_deref(), Some("law-of-demeter"));
    }

    #[test]
    fn does_not_flag_a_shallow_chain() {
        let source = "a.b.c\n";
        let tree = parse("python", source);
        assert!(scan(tree.root_node(), source, 3).is_empty());
    }

    #[test]
    fn does_not_flag_a_fluent_call_chain() {
        let source = "a.b().c().d().e()\n";
        let tree = parse("python", source);
        assert!(
            scan(tree.root_node(), source, 3).is_empty(),
            "call chains are a different smell"
        );
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let source = "# caf\u{e9}\u{e9}\nx = \u{e9}.b.c.d.e\n";
        let tree = parse("python", source);
        let findings = scan(tree.root_node(), source, 3);
        assert_eq!(findings.len(), 1);
        let span = findings[0].span.as_ref().expect("span");
        assert_eq!((span.start_line, span.start_col), (2, 5), "start after `x = `");
        assert_eq!((span.end_line, span.end_col), (2, 14));
    }

    #[test]
    fn non_ascii_before_the_chain_on_the_same_line_shifts_columns_by_characters() {
        let source = "s = \"\u{e9}\u{e9}\"; a.b.c.d.e\n";
        let tree = parse("python", source);
        let findings = scan(tree.root_node(), source, 3);
        let span = findings[0].span.as_ref().expect("span");
        assert_eq!(span.start_col, 11, "`s = \"éé\"; ` is 10 characters but 12 bytes");
    }
}
