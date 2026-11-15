//! Explicit-stack tree traversal shared by the structural quality rules.
//!
//! A generated file can parse to a tree hundreds of thousands of levels deep (a long left-associative
//! operator chain), and a recursive walk would overflow the worker stack, which aborts the whole run
//! rather than failing one file. This walk keeps its state on the heap.

use tree_sitter::Node;

/// Visit `root`'s subtree depth-first: `enter` before a node's children (pre-order), `leave` after
/// them. Both receive the node's ancestors, outermost first, so callers need no `Node::parent`
/// (which costs time proportional to the node's depth).
pub fn walk<'t>(
    root: Node<'t>,
    mut enter: impl FnMut(Node<'t>, &[Node<'t>]),
    mut leave: impl FnMut(Node<'t>, &[Node<'t>]),
) {
    let mut cursor = root.walk();
    let mut ancestors: Vec<Node<'t>> = Vec::new();
    loop {
        let node = cursor.node();
        enter(node, &ancestors);
        if cursor.goto_first_child() {
            ancestors.push(node);
            continue;
        }
        loop {
            leave(cursor.node(), &ancestors);
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
            ancestors.pop();
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

    fn recursive(node: Node, out: &mut Vec<String>) {
        out.push(format!("+{}", node.kind()));
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                recursive(cursor.node(), out);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
        }
        out.push(format!("-{}", node.kind()));
    }

    #[test]
    fn matches_the_recursive_enter_leave_order() {
        let tree = parse("rust", "fn f(a: i32) -> i32 { if a > 0 { a } else { -a } }\n");
        let mut expected = Vec::new();
        recursive(tree.root_node(), &mut expected);
        let events = std::cell::RefCell::new(Vec::new());
        walk(
            tree.root_node(),
            |n, _| events.borrow_mut().push(format!("+{}", n.kind())),
            |n, _| events.borrow_mut().push(format!("-{}", n.kind())),
        );
        assert_eq!(events.into_inner(), expected);
    }

    #[test]
    fn ancestors_are_outermost_first() {
        let tree = parse("python", "x = (1 + 2)\n");
        walk(
            tree.root_node(),
            |n, ancestors| {
                assert_eq!(ancestors.last().map(|p| p.id()), n.parent().map(|p| p.id()));
                assert_eq!(ancestors.len(), {
                    let mut depth = 0;
                    let mut cur = n;
                    while let Some(p) = cur.parent() {
                        depth += 1;
                        cur = p;
                    }
                    depth
                });
            },
            |_, _| {},
        );
    }
}
