//! Cross-cutting spell-checker backend: wraps the `typos` + `typos-dict` crates.
//!
//! Declared for zero languages (`languages() = &[]`). The registry appends
//! this engine to **every** language match arm, but the runner plans it only
//! when `[engines.typos] enabled = true` (v1: `[lint.typos] enabled = true`) is
//! set (`OPT_IN_ENGINES` in poly-core's registry). `poly migrate` writes that
//! key for v1 configs that relied on the old default-on behaviour.
//!
//! # Capabilities
//!
//! Lint only, and **never autofix**. Misspellings are reported at
//! [`Severity::Warning`] with the dictionary suggestion in the message, but carry
//! no [`Edit`](poly_engine_api::engine::Edit) — auto-correcting a typo silently rewrites identifiers, string
//! keys, and API names that only *look* like misspellings, which regresses code
//! far more often than it helps. A typo must be resolved by a human.
//!
//! # Version string
//!
//! Encodes both the `typos` tokeniser version and the `typos-dict` word-list
//! version because either changing would alter output and must bust the cache.
//!
//! # Dictionary
//!
//! Uses `typos_dict::WORD` directly. Locale variants (`typos-vars`) are
//! intentionally excluded — they classify valid spellings as typos depending
//! on locale preference, which is inappropriate for a general-purpose backend.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

use globset::GlobSet;
use unicase::UniCase;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, OptionKeys, OptionTable, OptionType, Severity, SourceFile,
};
use poly_engine_api::language::Language;
use poly_engine_api::line_index::LineIndex;
use poly_engine_api::options::OptionsExt;

/// Combined cache-key version: `typos` tokeniser + `typos-dict` word list,
/// plus a marker for the noise-suppression guards below. Bump whenever either
/// crate is updated OR the guard logic changes (it alters output).
const TYPOS_VERSION: &str = "0.10.44+dict-0.14.5+guards1+cfg3+warn-noautofix+builtins2+rulecodes2+cols1+extcase1";

/// Skip spell-checking files at least this large: generated/minified bundles
/// dominate by size and are pure noise word-by-word.
const MAX_SPELL_CHECK_BYTES: usize = 1 << 20;

/// Skip files containing any line at least this long: very long lines are a
/// reliable signal of minified/generated content (one 11.7 MB Plotly bundle in
/// the dry-run corpus produced ~5k false positives each), not hand-written
/// prose or code.
const MAX_LINE_BYTES: usize = 2_000;

/// Minimum length of a flagged token. Ultra-short corrections (two-letter
/// minified identifiers) are overwhelmingly noise rather than real typos, so
/// require at least this many bytes; common three-letter typos are kept.
const MIN_TYPO_LEN: usize = 3;

/// Words the built-in dictionary flags but that are correct in essentially any
/// codebase: established technical abbreviations and well-known OSS
/// tool/library/format names. Baking them in spares every repo from re-listing
/// them in `[lint.*.typos] extend_words` (they recur across the dry-run corpus).
/// Deliberately narrow — only tokens that are proper nouns (never a real
/// misspelling) or firmly established terms; project-specific jargon stays in the
/// per-repo config so it can't mask genuine typos elsewhere. All entries are
/// lowercase and compared case-insensitively.
static BUILTIN_VALID_WORDS: &[&str] = &[
    "ser",
    "mis",
    "flate",
    "fpr",
    "arange",
    "unparseable",
    "certifi",
    "onnx",
    "wasm",
    "tesseract",
    "pdfium",
    "pymupdf",
    "surrealdb",
    "mkdocs",
    "mkdocstrings",
    "rumdl",
];

/// Whether `lowercased` is a built-in always-valid word (see [`BUILTIN_VALID_WORDS`]).
fn is_builtin_valid_word(lowercased: &str) -> bool {
    BUILTIN_VALID_WORDS.contains(&lowercased)
}

/// Longest all-uppercase prefix a lint rule code may have (`ASYNC230` is the
/// longest ruff prefix at five). Anything longer is treated as a normal
/// SCREAMING_CASE identifier and stays spell-checked.
const MAX_RULE_CODE_LETTERS: usize = 5;

/// Longest digit suffix a lint rule code may have (`PLR0917`, `ASYNC230`).
const MAX_RULE_CODE_DIGITS: usize = 5;

/// Whether `token` has the shape of a lint rule code: an all-uppercase ASCII
/// prefix of at most [`MAX_RULE_CODE_LETTERS`] letters, optionally followed by up
/// to [`MAX_RULE_CODE_DIGITS`] digits — `CPY`, `CPY001`, `S310`, `RUF100`,
/// `PLR0917`, `ASYNC230`, `MD012`.
///
/// Linter configuration lists these by the hundred (ruff `lint.select`/`ignore`
/// and per-file-ignores, `# noqa:` suppressions, rumdl `disable`), and the
/// dictionary reads them as misspelled acronyms — `CPY` as `COPY`/`CPU` — so
/// every repo carrying such a config had to allowlist rule codes by hand. The
/// shape is deliberately narrow: a token containing a separator (`TEH_VALUE`) or
/// a longer uppercase run is not a rule code and stays spell-checked. ~keep
fn is_lint_rule_code(token: &str) -> bool {
    let letters = token.chars().take_while(char::is_ascii_uppercase).count();
    if letters == 0 || letters > MAX_RULE_CODE_LETTERS {
        return false;
    }
    let digits = &token[letters..];
    digits.len() <= MAX_RULE_CODE_DIGITS && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Cross-cutting spell-checker declares no tier-1 language ownership.
static LANGUAGES: &[Language] = &[];

/// Shared tokeniser — `Tokenizer::new()` is `const fn`, so this is safe.
static TOKENIZER: typos::tokens::Tokenizer = typos::tokens::Tokenizer::new();

/// Cross-cutting spell-checker backed by the published `typos`/`typos-dict` crates.
pub struct TyposEngine;

pub use poly_engine_api::host_hooks::typos_options::{ARRAY_OPTION_KEYS, MAP_OPTION_KEYS};

/// Every option key the typos backend reads, paired with the type it is read
/// as, in one slice for `Engine::option_keys`.
///
/// Built from the two typed lists above rather than restated, so a key can
/// never be declared under a type the merge does not read it as.
pub(crate) static OPTION_KEYS: std::sync::LazyLock<Vec<(&'static str, OptionType)>> = std::sync::LazyLock::new(|| {
    MAP_OPTION_KEYS
        .iter()
        .map(|key| (*key, OptionType::TABLE))
        .chain(ARRAY_OPTION_KEYS.iter().map(|key| (*key, OptionType::ARRAY)))
        .collect()
});

impl Engine for TyposEngine {
    fn overridden_hooks(&self) -> u32 {
        0
    }

    fn name(&self) -> &'static str {
        "typos"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: false,
            fix: false,
            project: false,
        }
    }

    /// The dictionary and exclusion keys, taken from the one list
    /// `Config::build_typos_options` merges (see [`MAP_OPTION_KEYS`]). The same
    /// set applies to `[lint.typos]` and to the per-language
    /// `[lint.<lang>.typos]` override; nothing else survives that merge, so
    /// anything else written there is dead config.
    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Lint | OptionTable::CrossCuttingLint => OptionKeys::declared(&OPTION_KEYS),
            OptionTable::Format => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        TYPOS_VERSION
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        if is_generated_or_minified(&src.content) {
            return Ok(Vec::new());
        }

        let prepared = prepared_for(cfg);
        if let Some(set) = &prepared.exclude
            && path_matches_set(&src.path, set)
        {
            return Ok(Vec::new());
        }

        let dict = ConfiguredDictionary {
            valid_words: &prepared.valid_words,
            valid_identifiers: &prepared.valid_identifiers,
            corrections: &prepared.word_corrections,
        };

        let masked = merge_ranges(
            prepared
                .ignore_re
                .iter()
                .flat_map(|re| re.find_iter(&src.content).map(|m| (m.start(), m.end())))
                .collect(),
        );

        let index = LineIndex::new(&src.content);
        let diags = typos::check_str(&src.content, &TOKENIZER, &dict)
            .filter(|typo| typo.typo.len() >= MIN_TYPO_LEN)
            .filter(|typo| !byte_in_ranges(typo.byte_offset, &masked))
            .filter(|typo| !prepared.word_ignore_re.iter().any(|re| re.is_match(typo.typo.as_ref())))
            .filter(|typo| {
                !prepared
                    .ident_ignore_re
                    .iter()
                    .any(|re| re.is_match(typo.typo.as_ref()))
            })
            .map(|typo| typo_to_diagnostic(&index, typo))
            .collect();
        Ok(diags)
    }
}

/// Everything derived from one [`EngineConfig`] that does not depend on the file
/// being checked, built once and shared across files.
struct Prepared {
    exclude: Option<GlobSet>,
    ignore_re: Vec<regex::Regex>,
    word_ignore_re: Vec<regex::Regex>,
    ident_ignore_re: Vec<regex::Regex>,
    /// Lowercased words treated as valid: `extend_ignore_words` plus `extend_words`
    /// entries whose value equals their key.
    valid_words: HashSet<String>,
    /// Lowercased `extend_identifiers` keys.
    valid_identifiers: HashSet<String>,
    /// `extend_words` entries whose value differs from the key: `(typo, correction)`,
    /// typo lowercased.
    word_corrections: Vec<(String, String)>,
}

/// Upper bound on memoized configs; the map is cleared when it grows past this.
const MAX_PREPARED_CONFIGS: usize = 64;

static PREPARED: LazyLock<Mutex<HashMap<[u8; 32], Arc<Prepared>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The memoized [`Prepared`] for `cfg`, keyed by its fingerprint.
fn prepared_for(cfg: &EngineConfig) -> Arc<Prepared> {
    let key = cfg.fingerprint();
    if let Ok(map) = PREPARED.lock()
        && let Some(found) = map.get(&key)
    {
        return Arc::clone(found);
    }
    let built = Arc::new(Prepared::build(&cfg.options));
    if let Ok(mut map) = PREPARED.lock() {
        if map.len() >= MAX_PREPARED_CONFIGS {
            map.clear();
        }
        map.insert(key, Arc::clone(&built));
    }
    built
}

impl Prepared {
    fn build(options: &toml::Table) -> Self {
        let exclude_globs = options.opt_string_list("extend_exclude");
        let exclude =
            (!exclude_globs.is_empty()).then(|| poly_engine_api::glob_set::build_glob_set_lossy(&exclude_globs));

        let mut valid_words: HashSet<String> = options
            .opt_string_list("extend_ignore_words")
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        let mut word_corrections = Vec::new();
        if let Some(words_table) = options.opt_table("extend_words") {
            for (typo, value) in words_table {
                match value.as_str() {
                    Some(correction) if !correction.eq_ignore_ascii_case(typo) => {
                        word_corrections.push((typo.to_ascii_lowercase(), correction.to_string()));
                    }
                    _ => {
                        valid_words.insert(typo.to_ascii_lowercase());
                    }
                }
            }
        }

        let valid_identifiers = options
            .opt_table("extend_identifiers")
            .map(|t| t.keys().map(|k| k.to_ascii_lowercase()).collect())
            .unwrap_or_default();

        Self {
            exclude,
            ignore_re: compile_regexes(&options.opt_string_list("extend_ignore_re")),
            word_ignore_re: compile_regexes(&options.opt_string_list("extend_ignore_words_re")),
            ident_ignore_re: compile_regexes(&options.opt_string_list("extend_ignore_identifiers_re")),
            valid_words,
            valid_identifiers,
            word_corrections,
        }
    }
}

/// Compile `patterns` into regexes, skipping (with a warning) any that fail to
/// compile so one malformed user pattern never aborts the whole run.
fn compile_regexes(patterns: &[String]) -> Vec<regex::Regex> {
    patterns
        .iter()
        .filter_map(|p| match regex::Regex::new(p) {
            Ok(re) => Some(re),
            Err(_) => {
                tracing::warn!(pattern = %p, engine = "typos", "invalid extend-ignore regex; skipping");
                None
            }
        })
        .collect()
}

/// Sort `ranges` and merge overlapping ones so [`byte_in_ranges`] can binary-search.
fn merge_ranges(mut ranges: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Whether `offset` falls within any `[start, end)` range of the sorted, disjoint `ranges`.
fn byte_in_ranges(offset: usize, ranges: &[(usize, usize)]) -> bool {
    let idx = ranges.partition_point(|&(start, _)| start <= offset);
    idx > 0 && offset < ranges[idx - 1].1
}

/// Whether `content` looks generated/minified and should not be spell-checked:
/// at least [`MAX_SPELL_CHECK_BYTES`] in size, or containing any line of at
/// least [`MAX_LINE_BYTES`]. Both are reliable signals of machine-emitted
/// bundles rather than hand-written prose or code.
fn is_generated_or_minified(content: &str) -> bool {
    content.len() >= MAX_SPELL_CHECK_BYTES || content.lines().any(|line| line.len() >= MAX_LINE_BYTES)
}

/// Return `true` when `path` matches `set`, a gitignore-style glob set.
/// Matching is tried against the full path first, then against each
/// successive suffix (leading component stripped repeatedly) so bare patterns
/// like `CHANGELOG.md` match `/repo/CHANGELOG.md` and `tests/**` matches
/// `/repo/tests/foo.rs`. A malformed pattern is skipped with a warning when the set is built.
fn path_matches_set(path: &std::path::Path, set: &GlobSet) -> bool {
    if set.is_match(path) {
        return true;
    }
    let mut tail: &std::path::Path = path;
    loop {
        let mut comps = tail.components();
        let Some(_) = comps.next() else { break };
        let rest = comps.as_path();
        if rest == tail || rest.as_os_str().is_empty() {
            break;
        }
        if set.is_match(rest) {
            return true;
        }
        tail = rest;
    }
    false
}

/// In-process dictionary wrapping `typos_dict::WORD`, extended with
/// caller-supplied valid words, identifiers and corrections.
///
/// The built-in dictionary is consulted first, without allocating; a miss means
/// the token is not a typo whether or not it is in the valid sets, so those are
/// only checked (case-insensitively) on a hit. `corrections` are the exception:
/// they name typos the dictionary may not know and are checked first.
///
/// Implements [`typos::Dictionary`] so it can be passed to [`typos::check_str`].
struct ConfiguredDictionary<'a> {
    /// Lowercased words the user wants treated as valid spellings.
    valid_words: &'a HashSet<String>,
    /// Lowercased identifier tokens the user wants treated as valid.
    valid_identifiers: &'a HashSet<String>,
    /// `(lowercased typo, correction)` pairs from `extend_words`.
    corrections: &'a [(String, String)],
}

impl typos::Dictionary for ConfiguredDictionary<'_> {
    fn correct_ident<'s>(&'s self, ident: typos::tokens::Identifier<'_>) -> Option<typos::Status<'s>> {
        match ident.token() {
            "O_WRONLY" | "dBA" => return Some(typos::Status::Valid),
            _ => {}
        }
        if is_lint_rule_code(ident.token()) {
            return Some(typos::Status::Valid);
        }
        if !self.valid_identifiers.is_empty() && self.valid_identifiers.contains(&ident.token().to_ascii_lowercase()) {
            return Some(typos::Status::Valid);
        }
        None
    }

    fn correct_word<'s>(&'s self, word_token: typos::tokens::Word<'_>) -> Option<typos::Status<'s>> {
        use typos::tokens::Case;

        if word_token.case() == Case::None {
            return None;
        }

        let token = word_token.token();
        if let Some((_, correction)) = self
            .corrections
            .iter()
            .find(|(typo, _)| typo.eq_ignore_ascii_case(token))
        {
            let mut adapted = Cow::Borrowed(correction.as_str());
            case_correct(&mut adapted, word_token.case());
            return Some(typos::Status::Corrections(vec![adapted]));
        }

        let word_case = UniCase::new(token);
        let corrections = typos_dict::WORD.find(&word_case).copied()?;

        let lowered = token.to_ascii_lowercase();
        if is_builtin_valid_word(&lowered) || self.valid_words.contains(&lowered) {
            return Some(typos::Status::Valid);
        }

        let mut status = if corrections.is_empty() {
            typos::Status::Invalid
        } else {
            typos::Status::Corrections(corrections.iter().map(|c| Cow::Borrowed(*c)).collect())
        };

        for s in status.corrections_mut() {
            case_correct(s, word_token.case());
        }

        Some(status)
    }
}

/// Adjust `correction` to match `case` (mirrors typos-cli's `case_correct`).
fn case_correct(correction: &mut Cow<'_, str>, case: typos::tokens::Case) {
    use typos::tokens::Case;
    match case {
        Case::Lower | Case::None => {}
        Case::Title => {
            let s = correction.to_mut();
            if !s.is_empty() {
                // SAFETY: ASCII-only index 0..1 on a non-empty ASCII string.
                s[0..1].make_ascii_uppercase();
            }
        }
        Case::Upper => {
            correction.to_mut().make_ascii_uppercase();
        }
    }
}

fn typo_to_diagnostic(index: &LineIndex<'_>, typo: typos::Typo<'_>) -> Diagnostic {
    let start_byte = typo.byte_offset;
    let end_byte = start_byte + typo.typo.len();

    let corrections: Vec<&str> = match &typo.corrections {
        typos::Status::Corrections(c) => c.iter().map(Cow::as_ref).collect(),
        _ => vec![],
    };

    let message = if corrections.is_empty() {
        format!("`{}` is a misspelling", typo.typo)
    } else {
        format!("`{}` should be `{}`", typo.typo, corrections.join("` or `"))
    };

    Diagnostic {
        engine: "typos".to_string(),
        code: Some("typo".to_string()),
        // Warning, not Error: a single dictionary false positive (a real word
        // inside an identifier, a short hex/base64 fragment, a domain term not in
        // the word list) must not fail CI. `poly lint` exits non-zero only on
        // Error severity, so spell-check findings surface without breaking builds.
        // Users who want hard failures can raise the severity in config.
        severity: Severity::Warning,
        title: message,
        description: None,
        url: None,
        span: Some(index.span(start_byte, end_byte)),
        fix: Vec::new(),
        metadata: Default::default(),
    }
}
