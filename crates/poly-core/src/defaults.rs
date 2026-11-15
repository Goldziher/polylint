//! Opinionated default behavior shared across backends.
//!
//! The base layer is whitespace normalization, applied by the reference backend
//! and reusable as a final pass for any formatter: normalize line endings, trim
//! trailing whitespace, collapse trailing blank lines, enforce a single final
//! newline.

use std::borrow::Cow;

use crate::config::GlobalDefaults;

/// Normalize whitespace per the global defaults. Idempotent.
pub fn normalize_whitespace(text: &str, g: &GlobalDefaults) -> String {
    normalize_whitespace_cow(text, g).into_owned()
}

/// [`normalize_whitespace`], borrowing `text` when it is already normalized.
pub fn normalize_whitespace_cow<'a>(text: &'a str, g: &GlobalDefaults) -> Cow<'a, str> {
    let nl = g.line_ending.as_str();
    let mut out = String::with_capacity(text.len() + nl.len());
    // Length of `out` through the last non-empty line, excluding its terminator: trailing blank
    // lines are dropped by truncating back to it.
    let mut content_end = 0;
    let mut rest = text;
    loop {
        let (mut line, next) = match rest.split_once('\n') {
            // A `\r` directly before `\n` is part of a CRLF terminator, not line content.
            Some((line, next)) => (line.strip_suffix('\r').unwrap_or(line), Some(next)),
            None => (rest, None),
        };
        if g.trim_trailing_whitespace {
            line = line.trim_end();
        }
        out.push_str(line);
        if !line.is_empty() {
            content_end = out.len();
        }
        let Some(next) = next else { break };
        out.push_str(nl);
        rest = next;
    }

    out.truncate(content_end);
    if g.final_newline && !out.is_empty() {
        out.push_str(nl);
    }
    if out == text {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LineEnding;

    /// The implementation this module shipped before the single-pass rewrite.
    fn reference(text: &str, g: &GlobalDefaults) -> String {
        let unified = text.replace("\r\n", "\n");
        let mut lines: Vec<String> = unified
            .split('\n')
            .map(|l| {
                if g.trim_trailing_whitespace {
                    l.trim_end().to_string()
                } else {
                    l.to_string()
                }
            })
            .collect();
        while matches!(lines.last(), Some(l) if l.is_empty()) {
            lines.pop();
        }
        let nl = g.line_ending.as_str();
        let mut out = lines.join(nl);
        if g.final_newline && !out.is_empty() {
            out.push_str(nl);
        }
        out
    }

    const INPUTS: &[&str] = &[
        "",
        "\n",
        "\n\n\n",
        "\r\n",
        "\r\n\r\n",
        "a",
        "a\n",
        "a\n\n\n",
        "a\r\nb\r\n",
        "a\r\nb\nc\r\n\r\n",
        "a  \n\tb\t\t\n   \n",
        "a \t \r\n b",
        "no final newline",
        "lead\n\n\nmid\n\n",
        "\n\nstarts blank",
        "a\u{a0}\nb\u{a0}\u{a0}",
        "a\u{c}\n\u{c}\n",
        "\u{2003}\n\u{3000}",
        "a\r",
        "a\r\r\nb",
        "a\r\r\n\r\r\n",
        "a\rb\n",
        "\r",
        "  ",
        "  \n  ",
        "h\u{e9}llo  \n\u{4e16}\u{754c}\t\n\n",
        "a\n  \n\nb",
    ];

    #[test]
    fn matches_the_reference_for_every_option_combination() {
        for ending in [LineEnding::Lf, LineEnding::Crlf] {
            for final_newline in [true, false] {
                for trim in [true, false] {
                    let g = GlobalDefaults {
                        line_ending: ending,
                        final_newline,
                        trim_trailing_whitespace: trim,
                        ..GlobalDefaults::default()
                    };
                    for input in INPUTS {
                        assert_eq!(
                            normalize_whitespace(input, &g),
                            reference(input, &g),
                            "input {input:?}, ending {ending:?}, final_newline {final_newline}, trim {trim}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn already_normalized_input_is_borrowed() {
        let g = GlobalDefaults::default();
        assert!(matches!(normalize_whitespace_cow("a\nb\n", &g), Cow::Borrowed(_)));
        assert!(matches!(normalize_whitespace_cow("a \nb\n", &g), Cow::Owned(_)));
        assert!(matches!(normalize_whitespace_cow("", &g), Cow::Borrowed("")));
    }

    #[test]
    fn is_idempotent() {
        let g = GlobalDefaults::default();
        for input in INPUTS {
            let once = normalize_whitespace(input, &g);
            assert_eq!(normalize_whitespace(&once, &g), once, "input {input:?}");
        }
    }
}
