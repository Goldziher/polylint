//! One traversal that feeds both `nesting-too-deep` and `cyclomatic-complexity`.
//!
//! Each metric keeps its own per-node logic ([`NestingWalk`], [`ComplexityWalk`]); sharing the walk
//! saves a full tree traversal and a kind lookup per node when both rules are on.

use std::cell::RefCell;

use tree_sitter::Node;

use super::complexity::{ComplexityFinding, ComplexityWalk};
use super::kinds::KindTable;
use super::nesting::{NestingFinding, NestingWalk};
use super::walk::walk;

/// Walk `root` once, running the nesting metric when `nesting` is `Some(threshold)` and the
/// complexity metric when `complexity` is `Some(threshold)`. Returns the findings of each, so the
/// caller keeps emitting every nesting finding before every complexity finding.
pub fn nesting_and_complexity(
    table: &KindTable,
    grammar: &str,
    root: Node,
    nesting: Option<usize>,
    complexity: Option<usize>,
) -> (Vec<NestingFinding>, Vec<ComplexityFinding>) {
    let state = RefCell::new((
        nesting.map(|threshold| NestingWalk::new(grammar, threshold)),
        complexity.map(ComplexityWalk::new),
    ));
    walk(
        root,
        |node, ancestors| {
            let info = table.info(node.kind_id());
            let mut guard = state.borrow_mut();
            let (nest, cx) = &mut *guard;
            if let Some(nest) = nest {
                nest.enter(node, ancestors, info.construct.nest);
            }
            if let Some(cx) = cx {
                cx.enter(node, &info);
            }
        },
        |node, _| {
            let info = table.info(node.kind_id());
            let mut guard = state.borrow_mut();
            let (nest, cx) = &mut *guard;
            if let Some(nest) = nest {
                nest.leave();
            }
            if let Some(cx) = cx {
                cx.leave(&info);
            }
        },
    );
    let (nest, cx) = state.into_inner();
    (
        nest.map(|n| n.findings).unwrap_or_default(),
        cx.map(|c| c.findings).unwrap_or_default(),
    )
}
