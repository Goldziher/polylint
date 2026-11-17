//! Guard against markup_fmt rewriting template-literal content in a script body.
//!
//! markup_fmt re-indents the script body it gets back from the external formatter line by line, and
//! keeps a line verbatim only while a per-line quote tracker says it is inside a template literal.
//! That tracker does not lex regular expressions, so a regex holding a quote or backtick
//! (`` /`([^`]+)`/g ``) inverts its state: the content of a later multi-line template literal is
//! re-indented, which changes the runtime string and grows on every pass, so `poly fmt` never
//! converges (<https://github.com/g-plane/markup_fmt/issues/280>).
//!
//! The guard compares the template literals poly's script formatter produced with those in the
//! script bodies markup_fmt extracts from its own output. Only bodies holding a multi-line template
//! literal are compared, since a single-line one is never split across the re-indented lines.

use std::borrow::Cow;

use markup_fmt::Language as MarkupLanguage;
use markup_fmt::config::FormatOptions;
use oxc_parser::config::RuntimeParserConfig;
use oxc_parser::{Kind, Parser};

use poly_engine_api::language::Language;

/// The decline reason for a file whose template-literal content markup_fmt would change.
pub(super) const REASON: &str = "markup_fmt would rewrite template-literal content in a <script> block \
     (https://github.com/g-plane/markup_fmt/issues/280)";

/// The template literals of each script body the external formatter returned, in call order.
#[derive(Default)]
pub(super) struct ScriptTemplates {
    /// One entry per script body: its template-literal tokens when one of them spans lines, else
    /// `None`, which needs no check.
    bodies: Vec<Option<Vec<String>>>,
}

impl ScriptTemplates {
    /// Record the script body handed back to markup_fmt, parsed as `ext`.
    pub(super) fn record(&mut self, body: &str, ext: &str) {
        let multi_line = if may_hold_multi_line_template(body) {
            template_tokens(body, ext).filter(|tokens| tokens.iter().any(|token| token.contains('\n')))
        } else {
            None
        };
        self.bodies.push(multi_line);
    }

    fn needs_check(&self) -> bool {
        self.bodies.iter().any(Option::is_some)
    }
}

/// Whether `output`, markup_fmt's result, keeps every multi-line template literal `recorded` saw.
///
/// markup_fmt is run over `output` again with an identity callback, so the callback sees each
/// script body exactly as it was written into `output`. The same classification as the format call
/// pairs those bodies with the recorded ones; a different count, a body that no longer parses, or a
/// template token that changed all fail the check.
pub(super) fn preserved(
    recorded: &ScriptTemplates,
    output: &str,
    language: MarkupLanguage,
    options: &FormatOptions,
    poly_language: &Language,
) -> bool {
    if !recorded.needs_check() {
        return true;
    }
    let mut seen = 0usize;
    let mut intact = true;
    let reparsed = markup_fmt::format_text(output, language, options, |code, hints| {
        if let Some(ext) = super::script_ext(poly_language, output, code, &hints) {
            if let Some(Some(expected)) = recorded.bodies.get(seen) {
                intact &= template_tokens(code, ext).is_some_and(|found| found == *expected);
            }
            seen += 1;
        }
        Ok(Cow::Borrowed(code))
    });
    reparsed.is_ok() && intact && seen == recorded.bodies.len()
}

/// Whether `code` can hold a template literal that spans lines: a newline between its first and last
/// backtick. Every template literal lies between those two, so this never misses one.
fn may_hold_multi_line_template(code: &str) -> bool {
    match (code.find('`'), code.rfind('`')) {
        (Some(first), Some(last)) => code.get(first..last).is_some_and(|between| between.contains('\n')),
        _ => false,
    }
}

/// The source text of every template-literal token of `code`, in order, with CRLF read as LF so a
/// configured line ending is not mistaken for a change; `None` when `code` does not parse.
fn template_tokens(code: &str, ext: &str) -> Option<Vec<String>> {
    super::super::oxc::with_arena(|allocator| {
        let parsed = Parser::new(allocator, code, super::super::oxc::source_type_for_extension(ext))
            .with_config(RuntimeParserConfig::new(true))
            .parse();
        if parsed.fatal_error {
            return None;
        }
        parsed
            .tokens
            .iter()
            .filter(|token| {
                matches!(
                    token.kind(),
                    Kind::NoSubstitutionTemplate | Kind::TemplateHead | Kind::TemplateMiddle | Kind::TemplateTail
                )
            })
            .map(|token| {
                code.get(token.start() as usize..token.end() as usize)
                    .map(|text| text.replace("\r\n", "\n"))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefilter_needs_a_newline_between_backticks() {
        assert!(!may_hold_multi_line_template("const a = 1;\nconst b = 2;\n"));
        assert!(!may_hold_multi_line_template("const a = `x`;\n"));
        assert!(!may_hold_multi_line_template("const a = `x\n"));
        assert!(may_hold_multi_line_template("const a = `\nx\n`;\n"));
    }

    #[test]
    fn template_tokens_skip_backticks_in_regexes_and_strings() {
        let code = "const re = /`([^`]+)`/g;\nconst s = \"`\";\nconst t = `a${b}\n  c`;\n";
        assert_eq!(
            template_tokens(code, "js"),
            Some(vec!["`a${".to_owned(), "}\n  c`".to_owned()])
        );
    }

    #[test]
    fn a_body_with_only_single_line_templates_needs_no_check() {
        let mut templates = ScriptTemplates::default();
        templates.record("const a = `x`;\nconst b = `y`;\n", "js");
        assert!(!templates.needs_check());
        templates.record("const a = `\nx\n`;\n", "js");
        assert!(templates.needs_check());
    }
}
