//! Guard for oxc-project/oxc#29733: `oxc_formatter_markdown` prints a footnote definition by indexing
//! its first child, so a definition with no content (`[^1]:` and nothing after it) panics the
//! formatter. Until upstream fixes it, such Markdown is never handed to the formatter.
//!
//! The check reads the AST the formatter itself would print, from the formatter's own
//! [`oxc_formatter_markdown::parse_for_format`], so it agrees with the printer on what is a footnote
//! definition: `[^1]:` inside a code fence is code, and `[^1]:` followed by an indented line holds that
//! line as its content.

use oxc_allocator::Allocator;
use oxc_markdown_parser::ast::Block;

/// Why a Markdown file holding an empty footnote definition is not formatted.
pub(crate) const EMPTY_FOOTNOTE_SKIP: &str = "an empty footnote definition (`[^label]:` with no content) crashes the \
     Markdown formatter; see https://github.com/oxc-project/oxc/issues/29733";

/// Whether formatting `text` as Markdown would reach an empty footnote definition.
///
/// Every footnote definition contains `[^` and `]:`, so text lacking either is answered without a
/// parse; only text that has both pays for a second parse, the formatter's being private to it.
/// Text the parser rejects answers `false`: the formatter fails the same parse and returns an error
/// before printing anything.
pub(crate) fn has_empty_footnote_definition(text: &str) -> bool {
    if !text.contains("[^") || !text.contains("]:") {
        return false;
    }
    let allocator = Allocator::new();
    oxc_formatter_markdown::parse_for_format(&allocator, text)
        .is_ok_and(|parsed| any_empty_footnote(&parsed.root.children))
}

/// Every block container the parser builds, searched for an empty footnote definition.
fn any_empty_footnote(blocks: &[Block<'_>]) -> bool {
    blocks.iter().any(|block| match block {
        Block::FootnoteDefinition(definition) => {
            definition.children.is_empty() || any_empty_footnote(&definition.children)
        }
        Block::Blockquote(quote) => any_empty_footnote(&quote.children),
        Block::List(list) => list.children.iter().any(|item| any_empty_footnote(&item.children)),
        Block::ContainerDirective(directive) => any_empty_footnote(&directive.children),
        Block::MdxJsx(element) => any_empty_footnote(&element.children),
        Block::Paragraph(_)
        | Block::Heading(_)
        | Block::ThematicBreak(_)
        | Block::CodeBlock(_)
        | Block::HtmlBlock(_)
        | Block::Definition(_)
        | Block::Table(_)
        | Block::MathBlock(_)
        | Block::Liquid(_)
        | Block::MdxEsm(_)
        | Block::MdxExpression(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::has_empty_footnote_definition;

    #[test]
    fn a_definition_with_nothing_after_the_colon_is_empty() {
        assert!(has_empty_footnote_definition("[^1]:\n"));
        assert!(has_empty_footnote_definition("[^1]:"));
        assert!(has_empty_footnote_definition("Text[^1].\n\n[^1]:   \n"));
    }

    #[test]
    fn an_empty_definition_inside_a_container_is_found() {
        assert!(has_empty_footnote_definition("> [^1]:\n"));
    }

    #[test]
    fn a_definition_with_content_is_not_empty() {
        assert!(!has_empty_footnote_definition("[^1]: Note.\n"));
        assert!(!has_empty_footnote_definition("[^1]:\n    Content.\n"));
    }

    #[test]
    fn an_empty_definition_inside_a_code_fence_is_code() {
        assert!(!has_empty_footnote_definition("```markdown\n[^1]:\n```\n"));
        assert!(!has_empty_footnote_definition("    [^1]:\n"));
    }

    #[test]
    fn text_without_a_definition_is_not_parsed_into_one() {
        assert!(!has_empty_footnote_definition(
            "# Title\n\nA [link](x) and a [^ref] mention.\n"
        ));
    }
}
