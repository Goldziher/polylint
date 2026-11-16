//! Opt-in prose and grammar checks backed by `harper-core`.
//!
//! Cross-cutting (`languages() == &[]`) and lint only. It reads two kinds of text: a Markdown file
//! as a whole, through harper's Markdown parser (code blocks, inline code and tables are skipped),
//! and the comments of every other language, found with poly's tree-sitter grammars and parsed as
//! Markdown too, since doc comments are written in it ([`crate::comments`]).
//!
//! Off unless `[engines.harper] enabled = true`. Findings are warnings and carry no autofix: a
//! grammar suggestion is a judgement, and rewriting prose silently is worse than reporting it. The
//! suggestions are listed in the diagnostic's description instead.
//!
//! # Rules
//!
//! The rule ids are harper's own (`AnA`, `RepeatedWords`, `SpellCheck`, ...), matched without regard
//! to case, `-` or `_`. `select` replaces the default set, `extend_select` adds to it and `ignore`
//! removes from it. The default set is harper's curated one minus `SpellCheck`: the `typos` engine
//! already owns spelling, and harper's dictionary flags the jargon every codebase is full of.
//! Selecting `SpellCheck` turns it back on.

use std::cell::RefCell;
use std::sync::Arc;

use harper_core::linting::{Lint, LintGroup};
use harper_core::parsers::MarkdownOptions;
use harper_core::spell::{FstDictionary, MergedDictionary, MutableDictionary};
use harper_core::{Dialect, DictWordMetadata, Document};

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, OptionType, Severity, SourceFile, Span,
};
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;
use poly_engine_api::options::OptionsExt;
use poly_engine_api::rule_config::RuleSelection;

use crate::comments;
use crate::text::{self, Mapped};

/// Cache-key identity: the `harper-core` version plus a marker for this backend's own mapping and
/// rule-selection logic. Bump when the crate moves or either changes.
const VERSION: &str = "harper-core-3.2.0+comments1+select1";

/// Files at least this large are generated or bundled content, not prose worth reading.
const MAX_BYTES: usize = 1 << 20;

/// The rule harper's own spelling dictionary runs under; off unless selected (see the module docs).
const SPELL_CHECK: &str = "SpellCheck";

/// Upper bound on the rule groups one worker thread keeps; building one is the expensive step.
const MAX_GROUPS: usize = 4;

/// The option keys the backend reads, beside the uniform rule-selection vocabulary.
static OPTION_KEYS: &[(&str, OptionType)] = &[
    ("dialect", OptionType::STRING),
    ("dictionary", OptionType::ARRAY),
    ("comments", OptionType::BOOLEAN),
];

/// The harper prose engine. See the module docs.
#[derive(Debug, Clone, Copy, Default)]
pub struct HarperEngine;

/// A configured harper: the rule set and the dictionary documents are parsed against.
struct Group {
    lints: LintGroup,
    dictionary: Arc<MergedDictionary>,
}

thread_local! {
    /// Rule groups keyed by config fingerprint. A group holds caches that are not `Sync`, so each
    /// worker builds its own once per configuration rather than once per file.
    static GROUPS: RefCell<Vec<([u8; 32], Group)>> = const { RefCell::new(Vec::new()) };
}

fn enabled(cfg: &EngineConfig) -> bool {
    cfg.options.opt_bool("enabled").unwrap_or(false)
}

impl Engine for HarperEngine {
    fn name(&self) -> &'static str {
        "harper"
    }

    fn languages(&self) -> &'static [Language] {
        &[]
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            ..Capabilities::default()
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint | OptionTable::CrossCuttingLint => {
                OptionKeys::declared(OPTION_KEYS).with_rule_selection()
            }
            OptionTable::Format => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn overridden_hooks(&self) -> u32 {
        poly_engine_api::desc::hooks::PROVIDES_LANGUAGE
    }

    /// Never claims a language: prose checks cover a file's text, not its code, so a green run
    /// says nothing about the language's lint coverage.
    fn provides_language_lint(&self, _language: &Language, _cfg: &EngineConfig) -> bool {
        false
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        if !enabled(cfg) || src.content.len() >= MAX_BYTES {
            return Ok(Vec::new());
        }
        let blocks = match src.language {
            Language::Markdown => vec![text::markdown_body(&src.content)],
            Language::Mdx => Vec::new(),
            _ if cfg.options.opt_bool("comments").unwrap_or(true) => comments::blocks(src),
            _ => Vec::new(),
        };
        if blocks.is_empty() {
            return Ok(Vec::new());
        }
        let index = LineIndex::new(&src.content);
        let fingerprint = cfg.fingerprint();
        let diagnostics = GROUPS.with(|cell| {
            let mut groups = cell.borrow_mut();
            let at = match groups.iter().position(|(key, _)| *key == fingerprint) {
                Some(at) => at,
                None => {
                    if groups.len() >= MAX_GROUPS {
                        groups.remove(0);
                    }
                    groups.push((fingerprint, build_group(cfg)));
                    groups.len() - 1
                }
            };
            let group = &mut groups[at].1;
            blocks
                .iter()
                .flat_map(|block| check_block(group, block, &index))
                .collect::<Vec<_>>()
        });
        Ok(diagnostics)
    }
}

/// One finding before it is mapped back to the file.
struct Finding {
    rule: String,
    lint: Lint,
}

fn check_block(group: &mut Group, block: &Mapped, index: &LineIndex<'_>) -> Vec<Diagnostic> {
    let document = Document::new_markdown(&block.text, MarkdownOptions::default(), &*group.dictionary);
    let mut findings: Vec<Finding> = group
        .lints
        .organized_lints(&document)
        .into_iter()
        .flat_map(|(rule, lints)| {
            lints.into_iter().map(move |lint| Finding {
                rule: rule.clone(),
                lint,
            })
        })
        .collect();
    // Overlapping findings say the same thing twice: keep the most important at each position.
    findings.sort_by(|a, b| {
        (a.lint.span.start, a.lint.priority, &a.rule).cmp(&(b.lint.span.start, b.lint.priority, &b.rule))
    });
    let chars: Vec<char> = block.text.chars().collect();
    let mut covered = 0;
    let mut diagnostics = Vec::new();
    for finding in findings {
        let span = &finding.lint.span;
        if span.start < covered || span.end <= span.start {
            continue;
        }
        let Some((start, end)) = block.byte_range(&chars, span.start, span.end) else {
            continue;
        };
        covered = span.end;
        diagnostics.push(to_diagnostic(index, &finding, start, end));
    }
    diagnostics
}

fn to_diagnostic(index: &LineIndex<'_>, finding: &Finding, start: usize, end: usize) -> Diagnostic {
    let (start_line, start_col) = index.line_col(start);
    let (end_line, end_col) = index.line_col(end);
    let suggestions: Vec<String> = finding.lint.suggestions.iter().map(ToString::to_string).collect();
    Diagnostic {
        engine: "harper".to_owned(),
        code: Some(finding.rule.clone()),
        severity: Severity::Warning,
        title: finding.lint.message.clone(),
        description: (!suggestions.is_empty()).then(|| suggestions.join("; ")),
        span: Some(Span {
            start_line,
            start_col,
            end_line,
            end_col,
        }),
        url: None,
        fix: Vec::new(),
        metadata: [("kind".to_owned(), finding.lint.lint_kind.to_string())].into(),
    }
}

fn build_group(cfg: &EngineConfig) -> Group {
    let mut merged = MergedDictionary::new();
    let words = cfg.options.opt_string_list("dictionary");
    if !words.is_empty() {
        let mut user = MutableDictionary::new();
        for word in &words {
            user.append_word_str(word, DictWordMetadata::default());
        }
        merged.add_dictionary(Arc::new(user));
    }
    merged.add_dictionary(FstDictionary::curated());
    let dictionary = Arc::new(merged);

    let mut lints = LintGroup::new_curated(Arc::clone(&dictionary), dialect(cfg));
    let selection = RuleSelection::from_options(cfg);
    let keys: Vec<String> = lints.iter_keys().map(str::to_owned).collect();
    for key in keys {
        let default_on = key != SPELL_CHECK && lints.config.is_rule_enabled(&key);
        lints
            .config
            .set_rule_enabled(&key, selected(&key, default_on, &selection));
    }
    Group { lints, dictionary }
}

/// Whether `rule` runs under `selection`, given whether it is in the default set.
fn selected(rule: &str, default_on: bool, selection: &RuleSelection) -> bool {
    let normalize = |id: &str| -> String {
        id.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let wanted = normalize(rule);
    let named = |ids: &[String]| ids.iter().any(|id| normalize(id) == wanted);
    let mut on = if selection.select.is_empty() {
        default_on
    } else {
        named(&selection.select)
    };
    on |= named(&selection.extend_select);
    on && !named(&selection.ignore)
}

fn dialect(cfg: &EngineConfig) -> Dialect {
    let Some(name) = cfg.options.opt_str("dialect") else {
        return Dialect::American;
    };
    match name.to_ascii_lowercase().replace('_', "-").as_str() {
        "american" | "us" | "en-us" => Dialect::American,
        "british" | "uk" | "gb" | "en-gb" => Dialect::British,
        "canadian" | "ca" | "en-ca" => Dialect::Canadian,
        "australian" | "au" | "en-au" => Dialect::Australian,
        "indian" | "in" | "en-in" => Dialect::Indian,
        other => {
            tracing::warn!(dialect = other, "unknown harper dialect; using american");
            Dialect::American
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(select: &[&str], extend: &[&str], ignore: &[&str]) -> RuleSelection {
        let owned = |ids: &[&str]| ids.iter().map(|id| (*id).to_owned()).collect();
        RuleSelection {
            select: owned(select),
            extend_select: owned(extend),
            ignore: owned(ignore),
            ..RuleSelection::default()
        }
    }

    #[test]
    fn select_replaces_extend_adds_and_ignore_removes() {
        assert!(selected("AnA", true, &selection(&[], &[], &[])));
        assert!(!selected("AnA", false, &selection(&[], &[], &[])));
        assert!(selected("SpellCheck", false, &selection(&[], &["spell-check"], &[])));
        assert!(!selected("AnA", true, &selection(&["RepeatedWords"], &[], &[])));
        assert!(selected(
            "RepeatedWords",
            false,
            &selection(&["repeated_words"], &[], &[])
        ));
        assert!(!selected("AnA", true, &selection(&[], &[], &["an_a"])));
    }

    #[test]
    fn never_claims_language_lint() {
        let cfg = EngineConfig {
            globals: poly_engine_api::config::GlobalDefaults::default(),
            indent_width: 4,
            options: toml::toml! { enabled = true },
        };
        for language in [Language::Markdown, Language::Rust, Language::Python] {
            assert!(!HarperEngine.provides_language_lint(&language, &cfg));
        }
    }
}
