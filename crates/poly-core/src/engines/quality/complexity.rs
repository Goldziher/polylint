//! `cyclomatic-complexity`: McCabe complexity per function-like scope, using
//! the same [`kinds`](super::kinds) construct table nesting uses.
//!
//! # Algorithm
//!
//! One explicit-stack walk of the whole file (see [`super::walk`]). A stack of `(count, span)` tracks
//! the currently open function-like scopes (McCabe base complexity 1 per
//! scope). Entering a [`NestRole::Reset`] boundary pushes a new scope;
//! `construct.decision` nodes increment the innermost open scope's count
//! (chained `else if`, each `case`/`catch` arm, and unconditional loops all
//! count here — unlike nesting, chaining does not exempt a decision point).
//! Leaving a scope emits a finding when its final count exceeds the
//! threshold, using the boundary node's own span.
//!
//! Decisions outside any function-like scope (bare module-level code) are
//! discarded — cyclomatic complexity is conventionally a per-function metric,
//! and a file-level number would be new noise this tier does not need.
//!
//! `&&`/`||` are folded into a single generic `binary_expression` (or
//! `binary`, in Ruby) node by most of these grammars rather than getting a
//! dedicated node kind, so they are matched directly on the operator token's
//! own tree-sitter kind (`"&&"`/`"||"`, i.e. the literal punctuation as its
//! own anonymous node) rather than through the per-grammar table — this is
//! the same check for every language.

use tree_sitter::Node;

use super::kinds::{self, KindInfo, KindTable, NestRole};

/// One cyclomatic-complexity finding for a single function-like scope.
pub struct ComplexityFinding {
    /// Final McCabe complexity of the scope (> threshold).
    pub complexity: usize,
    /// Byte range of the function-like boundary node, for the diagnostic span.
    pub start_byte: usize,
    /// See [`ComplexityFinding::start_byte`].
    pub end_byte: usize,
}

/// Walk `root` and collect every cyclomatic-complexity finding for `grammar`.
/// Returns an empty vec when `grammar` has no construct table.
pub fn analyze(grammar: &str, root: Node, threshold: usize) -> Vec<ComplexityFinding> {
    if !kinds::has_table(grammar) {
        return Vec::new();
    }
    let table = KindTable::build(grammar, &root.language());
    super::scan::nesting_and_complexity(&table, grammar, root, None, Some(threshold)).1
}

struct Scope {
    count: usize,
    start_byte: usize,
    end_byte: usize,
}

/// Incremental complexity state driven by [`super::scan`]'s shared traversal.
pub struct ComplexityWalk {
    threshold: usize,
    scopes: Vec<Scope>,
    /// One flag per open node: whether its `&&`/`||` child was already counted. A node counts once
    /// however many operator tokens it holds.
    bool_counted: Vec<bool>,
    /// Findings in the order scopes close.
    pub findings: Vec<ComplexityFinding>,
}

impl ComplexityWalk {
    /// Start an empty walk.
    pub fn new(threshold: usize) -> Self {
        Self {
            threshold,
            scopes: Vec::new(),
            bool_counted: Vec::new(),
            findings: Vec::new(),
        }
    }

    /// Called on entering `node`, described by `info`.
    ///
    /// The old per-node child scan for an operator token becomes a check on the token itself: when
    /// a `&&`/`||` child is entered, its parent is credited once. The innermost open scope at that
    /// moment is the one the parent's own entry would have credited, since every earlier sibling
    /// subtree has closed.
    pub fn enter(&mut self, node: Node, info: &KindInfo) {
        self.bool_counted.push(false);
        if info.construct.nest == NestRole::Reset {
            self.scopes.push(Scope {
                count: 1,
                start_byte: node.start_byte(),
                end_byte: node.end_byte(),
            });
        }
        if info.construct.decision {
            self.bump();
        }
        if info.bool_token
            && let Some(parent) = self.bool_counted.len().checked_sub(2)
            && !self.bool_counted[parent]
        {
            self.bool_counted[parent] = true;
            self.bump();
        }
    }

    /// Called on leaving the node last entered and not yet left, described by `info`.
    pub fn leave(&mut self, info: &KindInfo) {
        self.bool_counted.pop();
        if info.construct.nest != NestRole::Reset {
            return;
        }
        let scope = self.scopes.pop().expect("scope pushed on enter");
        if scope.count > self.threshold {
            self.findings.push(ComplexityFinding {
                complexity: scope.count,
                start_byte: scope.start_byte,
                end_byte: scope.end_byte,
            });
        }
    }

    fn bump(&mut self) {
        if let Some(top) = self.scopes.last_mut() {
            top.count += 1;
        }
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
    fn simple_function_has_base_complexity_one() {
        let tree = parse("rust", "fn f(a: i32) -> i32 {\n    a + 1\n}\n");
        assert!(
            analyze("rust", tree.root_node(), 1).is_empty(),
            "complexity 1 does not exceed threshold 1"
        );
    }

    #[test]
    fn each_branch_and_bool_operator_adds_one() {
        // 1 (base) + if + else-if + && + || = 5.
        let source = "fn f(a: i32, b: i32) -> i32 {\n    if a > 0 && b > 0 {\n        1\n    } else if a < 0 || b < 0 {\n        2\n    } else {\n        3\n    }\n}\n";
        let tree = parse("rust", source);
        let findings = analyze("rust", tree.root_node(), 4);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].complexity, 5);
    }

    #[test]
    fn nested_closure_gets_its_own_independent_scope() {
        let source = "fn f(a: i32) -> i32 {\n    if a > 0 {\n        let g = |x: i32| if x > 0 { x } else { -x };\n        g(a)\n    } else {\n        0\n    }\n}\n";
        let tree = parse("rust", source);
        // Outer: base 1 + if = 2 (below any reasonable threshold).
        // Closure: base 1 + if = 2, independently scoped.
        assert!(
            analyze("rust", tree.root_node(), 1).len() == 2,
            "both scopes exceed threshold 1"
        );
        assert!(
            analyze("rust", tree.root_node(), 2).is_empty(),
            "neither scope exceeds threshold 2"
        );
    }

    #[test]
    fn languages_without_a_construct_table_report_nothing() {
        let tree = parse(
            "php",
            "<?php\nfunction f($a) {\n  if ($a && $a > 0) {\n    return 1;\n  }\n  return 0;\n}\n",
        );
        assert!(analyze("php", tree.root_node(), 1).is_empty());
    }
}
