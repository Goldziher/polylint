//! Per-grammar node-kind classification.
//!
//! The raw `tree_sitter::Node::kind_id` is a `u16`; resolving it to a string and
//! running substring tests on every node of every walk is the expensive part of
//! the CST traversals. A [`KindTable`] evaluates those tests once per kind of a
//! grammar and is then indexed by id, so a walk is a table lookup per node.

use tree_sitter::{Language, Node};

/// Kind contains `string` or `comment`: a node whose interior is never reindented.
pub(super) const STRING_OR_COMMENT: u16 = 1 << 0;
/// Kind contains `heredoc`.
pub(super) const HEREDOC: u16 = 1 << 1;
/// Kind is `(`, `[` or `{`.
pub(super) const OPEN: u16 = 1 << 2;
/// Kind is `)`, `]` or `}`.
pub(super) const CLOSE: u16 = 1 << 3;
/// Rust only: kind is `token_tree`.
pub(super) const TOKEN_TREE: u16 = 1 << 4;
/// Rust only: kind is `macro_invocation`.
pub(super) const MACRO_INVOCATION: u16 = 1 << 5;
/// Kind is `:`.
pub(super) const COLON: u16 = 1 << 6;
/// Kind is exactly `comment`.
pub(super) const COMMENT: u16 = 1 << 7;
/// A switch case node whose label and body the grammar's case adjustment splits
/// (Swift `switch_entry`, Go `*_case`, Dart `switch_statement_case`/`_default`,
/// C# `switch_section`).
pub(super) const CASE_HOLDER: u16 = 1 << 8;
/// Go: a switch or select statement whose direct comment children introduce a case.
pub(super) const SWITCH_HOLDER: u16 = 1 << 9;
/// Go: `labeled_statement`.
pub(super) const LABELED: u16 = 1 << 10;
/// Swift: `statements`.
pub(super) const STATEMENTS: u16 = 1 << 11;

/// Classification flags for every kind id of one grammar.
pub(super) struct KindTable {
    flags: Vec<u16>,
}

impl KindTable {
    /// Classify every kind of `language`, with the `grammar`-specific flags for its name.
    pub(super) fn new(language: &Language, grammar: &str) -> Self {
        let flags = (0..language.node_kind_count())
            .map(|id| {
                u16::try_from(id)
                    .ok()
                    .and_then(|id| language.node_kind_for_id(id))
                    .map_or(0, |name| classify(name, grammar))
            })
            .collect();
        Self { flags }
    }

    /// Whether `node`'s kind carries `flag`. A kind outside the table (the `ERROR` node) has none.
    pub(super) fn has(&self, node: &Node, flag: u16) -> bool {
        self.flags
            .get(usize::from(node.kind_id()))
            .is_some_and(|f| f & flag != 0)
    }
}

/// The flags for a kind named `name` in `grammar`; the same string tests the
/// traversals used to run on every node.
fn classify(name: &str, grammar: &str) -> u16 {
    let mut flags = 0;
    if name.contains("string") || name.contains("comment") {
        flags |= STRING_OR_COMMENT;
    }
    if name.contains("heredoc") {
        flags |= HEREDOC;
    }
    match name {
        "(" | "[" | "{" => flags |= OPEN,
        ")" | "]" | "}" => flags |= CLOSE,
        ":" => flags |= COLON,
        "comment" => flags |= COMMENT,
        _ => {}
    }
    flags | grammar_flags(name, grammar)
}

fn grammar_flags(name: &str, grammar: &str) -> u16 {
    match (grammar, name) {
        ("rust", "token_tree") => TOKEN_TREE,
        ("rust", "macro_invocation") => MACRO_INVOCATION,
        ("swift", "switch_entry") => CASE_HOLDER,
        ("swift", "statements") => STATEMENTS,
        ("dart", "switch_statement_case" | "switch_statement_default") => CASE_HOLDER,
        ("csharp", "switch_section") => CASE_HOLDER,
        ("go", "expression_case" | "default_case" | "type_case" | "communication_case") => CASE_HOLDER,
        ("go", "expression_switch_statement" | "type_switch_statement" | "select_statement") => SWITCH_HOLDER,
        ("go", "labeled_statement") => LABELED,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(grammar: &str) -> (Language, KindTable) {
        let language = crate::engines::grammar::language(grammar).expect("grammar must load");
        let table = KindTable::new(&language, grammar);
        (language, table)
    }

    /// Every kind id of the grammar, so the assertions cannot pass over an empty table.
    fn all_kinds(language: &Language) -> Vec<(u16, String)> {
        let kinds: Vec<(u16, String)> = (0..language.node_kind_count())
            .filter_map(|id| {
                let id = u16::try_from(id).ok()?;
                Some((id, language.node_kind_for_id(id)?.to_string()))
            })
            .collect();
        assert!(kinds.len() > 50, "grammar must expose its kinds");
        kinds
    }

    #[test]
    fn string_comment_flag_equals_the_substring_predicate_for_every_kind() {
        for grammar in ["rust", "go", "javascript"] {
            let (language, table) = table(grammar);
            let mut protected = 0;
            for (id, name) in all_kinds(&language) {
                let old = name.contains("string") || name.contains("comment");
                let new = table.flags[usize::from(id)] & STRING_OR_COMMENT != 0;
                assert_eq!(new, old, "{grammar}: kind {id} `{name}`");
                protected += usize::from(old);
            }
            assert!(protected > 0, "{grammar} must have string or comment kinds");
        }
    }

    #[test]
    fn known_kinds_are_classified() {
        let (language, rust) = table("rust");
        let id_of = |name: &str| {
            all_kinds(&language)
                .into_iter()
                .find(|(_, n)| n == name)
                .unwrap_or_else(|| panic!("rust has no kind `{name}`"))
                .0
        };
        for name in ["string_literal", "raw_string_literal", "line_comment", "block_comment"] {
            assert!(rust.flags[usize::from(id_of(name))] & STRING_OR_COMMENT != 0, "{name}");
        }
        assert_eq!(rust.flags[usize::from(id_of("{"))], OPEN);
        assert_eq!(rust.flags[usize::from(id_of("}"))], CLOSE);
        assert!(rust.flags[usize::from(id_of("token_tree"))] & TOKEN_TREE != 0);
        assert!(rust.flags[usize::from(id_of("macro_invocation"))] & MACRO_INVOCATION != 0);
        assert_eq!(rust.flags[usize::from(id_of("function_item"))], 0);

        let (language, go) = table("go");
        let go_id = |name: &str| {
            all_kinds(&language)
                .into_iter()
                .find(|(_, n)| n == name)
                .unwrap_or_else(|| panic!("go has no kind `{name}`"))
                .0
        };
        assert_eq!(go.flags[usize::from(go_id("comment"))], COMMENT | STRING_OR_COMMENT);
        assert_eq!(go.flags[usize::from(go_id("expression_case"))], CASE_HOLDER);
        assert_eq!(go.flags[usize::from(go_id("labeled_statement"))], LABELED);
    }

    #[test]
    fn grammar_specific_flags_do_not_leak_across_grammars() {
        let (language, go) = table("go");
        assert!(
            (0..language.node_kind_count()).all(|id| go.flags[id] & (TOKEN_TREE | MACRO_INVOCATION | STATEMENTS) == 0),
            "go must carry no rust or swift flags"
        );
    }
}
