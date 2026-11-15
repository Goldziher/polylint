//! The shared-traversal, kind-table implementation must report exactly what the per-metric,
//! string-classifying walks it replaced reported. The reference functions below are those walks.

use std::cell::RefCell;

use tree_sitter::Node;

use super::complexity::{self, ComplexityFinding};
use super::demeter;
use super::kinds::{self, KindTable, NestRole};
use super::magic_number;
use super::nesting::{self, NestingFinding};
use super::scan::nesting_and_complexity;
use super::walk::walk;

fn parse(grammar: &str, source: &str) -> tree_sitter::Tree {
    let language = tree_sitter_language_pack::get_language(grammar).expect("grammar available");
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&language).expect("set_language");
    parser.parse(source, None).expect("parse")
}

fn reference_nesting(grammar: &str, root: Node, threshold: usize) -> Vec<(usize, usize, usize)> {
    let depths = RefCell::new(Vec::<usize>::new());
    let mut findings = Vec::new();
    walk(
        root,
        |node, ancestors| {
            let mut depths = depths.borrow_mut();
            let depth = depths.last().copied().unwrap_or(0);
            let next = match kinds::classify(grammar, node.kind()).nest {
                NestRole::Increment => depth + 1,
                NestRole::IncrementUnlessChained => {
                    if nesting::is_chained_else_if(node, ancestors, grammar) {
                        depth
                    } else {
                        depth + 1
                    }
                }
                NestRole::Same | NestRole::Ignore => depth,
                NestRole::Reset => 0,
            };
            depths.push(next);
            if next > threshold && next > depth {
                findings.push((next, node.start_byte(), node.end_byte()));
            }
        },
        |_, _| {
            depths.borrow_mut().pop();
        },
    );
    findings
}

fn has_bool_operator_child(node: Node) -> bool {
    let mut cursor = node.walk();
    if !cursor.goto_first_child() {
        return false;
    }
    loop {
        if kinds::is_bool_operator_text(cursor.node().kind()) {
            return true;
        }
        if !cursor.goto_next_sibling() {
            return false;
        }
    }
}

fn reference_complexity(grammar: &str, root: Node, threshold: usize) -> Vec<(usize, usize, usize)> {
    // (count, start, end) per open scope.
    let scopes = RefCell::new(Vec::<(usize, usize, usize)>::new());
    let findings = RefCell::new(Vec::new());
    let bump = |scopes: &mut Vec<(usize, usize, usize)>| {
        if let Some(top) = scopes.last_mut() {
            top.0 += 1;
        }
    };
    walk(
        root,
        |node, _| {
            let mut scopes = scopes.borrow_mut();
            let construct = kinds::classify(grammar, node.kind());
            if construct.nest == NestRole::Reset {
                scopes.push((1, node.start_byte(), node.end_byte()));
            }
            if construct.decision {
                bump(&mut scopes);
            }
            if has_bool_operator_child(node) {
                bump(&mut scopes);
            }
        },
        |node, _| {
            if kinds::classify(grammar, node.kind()).nest != NestRole::Reset {
                return;
            }
            let scope = scopes.borrow_mut().pop().expect("scope pushed on enter");
            if scope.0 > threshold {
                findings.borrow_mut().push((scope.0, scope.1, scope.2));
            }
        },
    );
    findings.into_inner()
}

fn nesting_tuples(findings: &[NestingFinding]) -> Vec<(usize, usize, usize)> {
    findings.iter().map(|f| (f.depth, f.start_byte, f.end_byte)).collect()
}

fn complexity_tuples(findings: &[ComplexityFinding]) -> Vec<(usize, usize, usize)> {
    findings
        .iter()
        .map(|f| (f.complexity, f.start_byte, f.end_byte))
        .collect()
}

fn python_source() -> String {
    let mut out = String::new();
    for i in 0..120 {
        out.push_str(&format!(
            "class C{i}:\n    def m{i}(self, a, b, c):\n        for x in a:\n            if x > {i} and b or c:\n                while x:\n                    if x == 1:\n                        try:\n                            f = lambda y: y if y and x else None\n                        except ValueError:\n                            pass\n            elif x < 0 or b:\n                with c:\n                    pass\n        return a.b.c.d.e + {}\n\n",
            i + 41
        ));
    }
    out
}

fn rust_source() -> String {
    let mut out = String::new();
    for i in 0..110 {
        out.push_str(&format!(
            "fn f{i}(a: i32, b: bool) -> i32 {{\n    let mut t = {};\n    let _z = s.p.q;\n    for x in 0..a {{\n        if x > 3 && b || x < -3 {{\n            while t > 0 {{\n                match x {{\n                    1 => {{ if b {{ t -= 1; }} }}\n                    _ => {{ let g = |y: i32| if y > 0 && b {{ y }} else {{ 0 }}; t = g(t); }}\n                }}\n            }}\n        }} else if x == 2 {{\n            t += 1;\n        }}\n    }}\n    t\n}}\n\n",
            i + 41
        ));
    }
    out
}

#[test]
fn shared_traversal_matches_the_per_metric_reference_walks() {
    for (grammar, source) in [("python", python_source()), ("rust", rust_source())] {
        assert!(
            source.lines().count() >= 1500,
            "{grammar}: source is too small to be meaningful"
        );
        let tree = parse(grammar, &source);
        let root = tree.root_node();
        let language = tree_sitter_language_pack::get_language(grammar).unwrap();
        let table = KindTable::build(grammar, &language);

        for (nest_threshold, cx_threshold) in [(1, 2), (2, 3), (4, 6), (4, 20)] {
            let expected_nesting = reference_nesting(grammar, root, nest_threshold);
            let expected_complexity = reference_complexity(grammar, root, cx_threshold);
            if (nest_threshold, cx_threshold) == (2, 3) {
                assert!(!expected_nesting.is_empty(), "{grammar}: reference found no nesting");
                assert!(
                    !expected_complexity.is_empty(),
                    "{grammar}: reference found no complexity"
                );
            }

            let (nest, cx) = nesting_and_complexity(&table, grammar, root, Some(nest_threshold), Some(cx_threshold));
            assert_eq!(nesting_tuples(&nest), expected_nesting, "{grammar}: combined nesting");
            assert_eq!(
                complexity_tuples(&cx),
                expected_complexity,
                "{grammar}: combined complexity"
            );

            assert_eq!(
                nesting_tuples(&nesting::analyze(grammar, root, nest_threshold)),
                expected_nesting,
                "{grammar}: standalone nesting"
            );
            assert_eq!(
                complexity_tuples(&complexity::analyze(grammar, root, cx_threshold)),
                expected_complexity,
                "{grammar}: standalone complexity"
            );
        }
    }
}

#[test]
fn demeter_and_magic_number_match_their_string_predicates() {
    for (grammar, source) in [("python", python_source()), ("rust", rust_source())] {
        let tree = parse(grammar, &source);
        let root = tree.root_node();

        let mut expected_numbers = Vec::new();
        let mut expected_chains = Vec::new();
        walk(
            root,
            |node, ancestors| {
                let kind = node.kind().to_ascii_lowercase();
                let is_numeric = !["predefined_type", "type_identifier", "integral_type"].contains(&node.kind())
                    && ["int", "float", "number"].iter().any(|m| kind.contains(m));
                if node.child_count() == 0 && is_numeric {
                    let text = node.utf8_text(source.as_bytes()).unwrap();
                    let normalized = text.trim().trim_end_matches(['u', 'U', 'l', 'L', 'i', 'f']);
                    if normalized.replace('_', "").parse::<i64>().is_ok() {
                        expected_numbers.push(node.start_position());
                    }
                }
                let is_chain = |k: &str| {
                    let k = k.to_ascii_lowercase();
                    [
                        "member_expression",
                        "field_expression",
                        "attribute",
                        "selector_expression",
                    ]
                    .iter()
                    .any(|m| k.contains(m))
                };
                if is_chain(node.kind()) && !ancestors.last().is_some_and(|p| is_chain(p.kind())) {
                    expected_chains.push(node.start_position());
                }
            },
            |_, _| {},
        );

        let numbers: Vec<_> = magic_number::scan(root, source.as_bytes(), &[])
            .iter()
            .map(|d| d.span.as_ref().map(|s| (s.start_line, s.start_col)))
            .collect();
        let expected: Vec<_> = expected_numbers
            .iter()
            .map(|p| Some((p.row as u32 + 1, p.column as u32 + 1)))
            .collect();
        assert!(!expected.is_empty(), "{grammar}: no numeric literals found");
        assert_eq!(numbers, expected, "{grammar}: magic numbers");

        let chains: Vec<_> = demeter::scan(root, 0)
            .iter()
            .map(|d| d.span.as_ref().map(|s| (s.start_line, s.start_col)))
            .collect();
        let expected: Vec<_> = expected_chains
            .iter()
            .map(|p| Some((p.row as u32 + 1, p.column as u32 + 1)))
            .collect();
        assert!(!expected.is_empty(), "{grammar}: no access chains found");
        assert_eq!(chains, expected, "{grammar}: law-of-demeter chains");
    }
}
