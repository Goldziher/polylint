//! Prose blocks lifted out of source-file comments.
//!
//! harper reads prose, not code, so a file's comments are found with the tree-sitter grammar poly
//! already loads for the language, the comment markers are dropped, and runs of adjacent
//! whole-line comments are joined into one block so a sentence wrapped over several `//` lines is
//! read as one. Every character keeps the byte offset it came from ([`Mapped`]).

use std::cell::RefCell;
use std::collections::HashMap;

use tree_sitter::{Node, Parser};

use poly_engine_api::engine::SourceFile;

use crate::text::Mapped;

/// A block with fewer words than this is a label, not a sentence, and is left alone.
const MIN_WORDS: usize = 3;

/// Leading words that mark a comment as a tool directive rather than prose, compared lowercase.
const DIRECTIVES: &[&str] = &[
    "poly:",
    "noqa",
    "nolint",
    "pylint:",
    "ruff:",
    "mypy:",
    "type:",
    "pragma",
    "eslint-",
    "eslint:",
    "biome-ignore",
    "prettier-ignore",
    "istanbul",
    "rubocop:",
    "clippy::",
    "spdx-",
    "@ts-",
    "~keep",
    "-*-",
    "coding:",
    "frozen_string_literal:",
    "shellcheck",
    "region",
    "endregion",
];

thread_local! {
    /// One parser per grammar per worker thread, like every other tree-sitter user in poly.
    static PARSERS: RefCell<HashMap<String, Parser>> = RefCell::new(HashMap::new());
}

/// One comment node: its trimmed byte range and the rows it spans.
struct Comment {
    start: usize,
    end: usize,
    first_row: usize,
    last_row: usize,
    /// Nothing but whitespace precedes it on its first line.
    whole_line: bool,
}

/// The prose blocks in `src`'s comments; empty when the language has no loadable grammar.
pub(crate) fn blocks(src: &SourceFile) -> Vec<Mapped> {
    let grammar = src.language.id();
    let Some(language) = poly_grammar::language(grammar) else {
        return Vec::new();
    };
    let content: &str = &src.content;
    let comments = PARSERS.with(|cell| {
        let mut pool = cell.borrow_mut();
        if !pool.contains_key(grammar) {
            let mut parser = Parser::new();
            parser.set_language(&language).ok()?;
            pool.insert(grammar.to_owned(), parser);
        }
        let parser = pool.get_mut(grammar)?;
        // Another linked engine in this lint pass may already hold the tree.
        let tree = poly_grammar::shared_tree(&src.content, &language, || parser.parse(content.as_bytes(), None))?;
        Some(collect(tree.root_node(), content))
    });
    group(content, comments.unwrap_or_default())
}

/// Every comment node under `root`, in source order, without descending into one.
fn collect(root: Node<'_>, content: &str) -> Vec<Comment> {
    let mut found = Vec::new();
    let mut cursor = root.walk();
    'walk: loop {
        let node = cursor.node();
        if node.is_named() && node.kind().contains("comment") {
            let start = node.start_byte();
            let text = content.get(start..node.end_byte()).unwrap_or_default();
            let trimmed = text.trim_end();
            let line_start = content[..start].rfind('\n').map_or(0, |at| at + 1);
            let first_row = node.start_position().row;
            found.push(Comment {
                start,
                end: start + trimmed.len(),
                first_row,
                last_row: first_row + trimmed.matches('\n').count(),
                whole_line: content[line_start..start].trim().is_empty(),
            });
        } else if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'walk;
            }
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    found
}

/// Join runs of adjacent whole-line comments into blocks and drop the ones that are not prose.
fn group(content: &str, comments: Vec<Comment>) -> Vec<Mapped> {
    let mut blocks = Vec::new();
    // The block being built, the row it ends on, and whether the next comment may join it.
    let mut current: Option<(Mapped, usize, bool)> = None;
    for comment in &comments {
        let joins = matches!(&current, Some((_, last, true)) if comment.whole_line && comment.first_row == last + 1);
        if !joins {
            if let Some((block, ..)) = current.take() {
                finish(block, &mut blocks);
            }
            current = Some((Mapped::default(), 0, false));
        }
        if let Some((block, last, joinable)) = current.as_mut() {
            push_comment(block, content, comment);
            *last = comment.last_row;
            *joinable = comment.whole_line;
        }
    }
    if let Some((block, ..)) = current {
        finish(block, &mut blocks);
    }
    blocks
}

fn finish(block: Mapped, blocks: &mut Vec<Mapped>) {
    if block.text.split_whitespace().count() >= MIN_WORDS {
        blocks.push(block);
    }
}

/// Append `comment`'s prose lines to `block`, one `\n`-terminated line per source line.
fn push_comment(block: &mut Mapped, content: &str, comment: &Comment) {
    let mut at = comment.start;
    for raw in content[comment.start..comment.end].split('\n') {
        let line_end = at + raw.len();
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some((from, to)) = prose_range(line) {
            block.push_slice(&line[from..to], at + from);
        }
        block.push_synthetic('\n', line_end);
        at = line_end + 1;
    }
}

/// The byte range of `line` left once comment markers are dropped, or `None` when the line is a
/// directive, a shebang or looks like code rather than prose.
fn prose_range(line: &str) -> Option<(usize, usize)> {
    let lead = line.len() - line.trim_start().len();
    let rest = &line[lead..];
    if rest.starts_with("#!") {
        return None;
    }
    let mut from = lead + marker_len(rest);
    if line[from..].starts_with(' ') {
        from += 1;
    }
    let head = line.trim_end();
    let to = if let Some(inner) = head.strip_suffix("*/") {
        inner.trim_end_matches('*').trim_end().len()
    } else if let Some(inner) = head.strip_suffix("-->") {
        inner.trim_end().len()
    } else {
        head.len()
    };
    let from = from.min(to);
    let prose = line[from..to].trim();
    (!is_directive(prose) && !looks_like_code(prose)).then_some((from, to))
}

/// The length of the comment opener at the start of `rest`, `0` when it has none.
fn marker_len(rest: &str) -> usize {
    let run = |from: usize, ch: char| from + rest[from..].chars().take_while(|c| *c == ch).count();
    let bang = |at: usize| at + usize::from(rest[at..].starts_with('!'));
    if rest.starts_with("/*") {
        bang(run(2, '*'))
    } else if rest.starts_with("//") {
        bang(run(2, '/'))
    } else if rest.starts_with("<!--") {
        4
    } else if rest.starts_with('#') {
        run(1, '#')
    } else if rest.starts_with("--") {
        run(2, '-')
    } else if rest.starts_with(';') {
        run(1, ';')
    } else if rest.starts_with('%') {
        run(1, '%')
    } else if rest.starts_with('*') && !rest.starts_with("*/") {
        1
    } else {
        0
    }
}

fn is_directive(prose: &str) -> bool {
    let lowered = prose.to_ascii_lowercase();
    DIRECTIVES.iter().any(|directive| lowered.starts_with(directive))
}

/// A line that ends the way a statement does reads as commented-out code, not prose.
fn looks_like_code(prose: &str) -> bool {
    prose.ends_with([';', '{', '}'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prose(line: &str) -> Option<&str> {
        prose_range(line).map(|(from, to)| &line[from..to])
    }

    #[test]
    fn markers_are_dropped() {
        assert_eq!(prose("    /// Returns the value."), Some("Returns the value."));
        assert_eq!(prose("# a note here"), Some("a note here"));
        assert_eq!(prose(" * continued text */"), Some("continued text"));
        assert_eq!(prose("/** Summary line. */"), Some("Summary line."));
        assert_eq!(prose("<!-- note about it -->"), Some("note about it"));
    }

    #[test]
    fn directives_shebangs_and_code_are_not_prose() {
        assert_eq!(prose("#!/usr/bin/env python"), None);
        assert_eq!(prose("# noqa: E501"), None);
        assert_eq!(prose("// SPDX-License-Identifier: MIT"), None);
        assert_eq!(prose("// let x = compute();"), None);
    }

    #[test]
    fn adjacent_whole_line_comments_form_one_block() {
        let content = "// one two\n// three four\n\n// five six seven\nfn x() {}\n";
        let comments = vec![
            Comment {
                start: 0,
                end: 10,
                first_row: 0,
                last_row: 0,
                whole_line: true,
            },
            Comment {
                start: 11,
                end: 24,
                first_row: 1,
                last_row: 1,
                whole_line: true,
            },
            Comment {
                start: 26,
                end: 43,
                first_row: 3,
                last_row: 3,
                whole_line: true,
            },
        ];
        let blocks = group(content, comments);
        let texts: Vec<&str> = blocks.iter().map(|block| block.text.as_str()).collect();
        assert_eq!(texts, ["one two\nthree four\n", "five six seven\n"]);
    }
}
