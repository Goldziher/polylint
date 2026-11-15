//! The ast-grep root for one file, built on the lint pass's shared tree (ADR 0057).

use std::sync::Arc;

use ast_grep_core::AstGrep;
use ast_grep_core::tree_sitter::{LanguageExt, StrDoc};

use crate::language::TslpLanguage;

/// Parse `content` for `language`, or reuse the tree another linked engine already
/// parsed for it in this lint pass.
///
/// The parse itself stays `StrDoc::try_new`, so `ast-grep-core`'s own per-thread
/// parser pool (`PARSER_CACHE`) still does the work and poly keeps no parser of its
/// own here. A shared tree comes from a from-scratch parse of the same bytes with the
/// same `tree_sitter::Language`, so the root built on it is the one `try_new` would
/// have built.
pub(crate) fn root(content: &Arc<str>, language: TslpLanguage) -> Result<AstGrep<StrDoc<TslpLanguage>>, String> {
    let ts_language = language.get_ts_language();
    let mut parsed: Option<StrDoc<TslpLanguage>> = None;
    let mut failure: Option<String> = None;
    let tree = poly_grammar::shared_tree(content, &ts_language, || {
        match StrDoc::try_new(content, language.clone()) {
            Ok(doc) => {
                let tree = doc.tree.clone();
                parsed = Some(doc);
                Some(tree)
            }
            Err(error) => {
                failure = Some(error);
                None
            }
        }
    });
    let doc = match (parsed, tree) {
        (Some(doc), _) => doc,
        (None, Some(tree)) => StrDoc {
            src: content.to_string(),
            lang: language,
            tree,
        },
        (None, None) => return Err(failure.unwrap_or_else(|| "tree-sitter returned no tree".to_owned())),
    };
    Ok(AstGrep::doc(doc))
}
