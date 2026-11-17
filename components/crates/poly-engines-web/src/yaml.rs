//! YAML backend: validity lint via `serde_yaml` (libyaml), Prettier-compatible format via
//! `oxc_formatter_yaml`.
//!
//! Capabilities: [`Capabilities::lint`] + [`Capabilities::format`]. No fix.
//!
//! # Lint
//! Scans every document of the stream with `serde_yaml`, a port of libyaml, so exactly what
//! PyYAML and libyaml accept is accepted here too (multi-line quoted scalars continued at the
//! parent's indent, flow collections closed at the parent key's column). Contents are
//! discarded; only scanner and parser errors matter. A failure becomes a single [`Diagnostic`]
//! with code `"syntax"` and [`Severity::Error`] carrying the 1-based line/column.
//!
//! A Helm-style Go-templated file (`name: {{ .Release.Name }}`, a line starting with `{{- if … }}`) is
//! skipped by lint and format alike; template text that is inert YAML (quoted, in a block scalar, in a
//! comment, or inside a plain scalar that starts with other text) is not.
//!
//! # Format
//! Delegates to the formatter oxfmt itself uses (`oxc_formatter_yaml`), so a repository
//! formatted by oxfmt is a fixed point and the two tools agree byte for byte.
//!
//! ## Options layering
//! oxfmt defaults → `[defaults]` globals (`line_length`, `line_ending`) → the project's
//! `.oxfmtrc.json[c]` (see [`poly_engine_api::host_hooks::oxfmt_native`]) → user `[fmt.yaml.yaml]` table. Recognised
//! keys: `indent_width`, `line_width`, `line_ending`, `single_quote`, `bracket_spacing`,
//! `trailing_commas` (`"always"` / `"never"`) and `prose_wrap` (`"preserve"` / `"always"` /
//! `"never"`).
//!
//! Returns [`FormatOutput::Unchanged`] when the output equals the input or the formatter declines the file.

use oxc_allocator::Allocator;
use oxc_formatter_core::FormatOptions;
use oxc_formatter_yaml::{ProseWrap, TrailingCommas, YamlFormatOptions};
use serde::Deserialize as _;
use std::borrow::Cow;

use crate::core_options::core_format_options;
use poly_engine_api::config::EngineConfig;
use poly_engine_api::engine::{
    Capabilities, Diagnostic, Engine, FormatOutput, OptionKeys, OptionTable, OptionType, Severity, SourceFile, Span,
};
use poly_engine_api::language::Language;
use poly_engine_api::options::OptionsExt;
use poly_engine_api::template::{GO_TEMPLATE_SKIP, contains_go_template};

/// Flow-collection depth beyond which a document is validated before it is
/// formatted. Valid documents nest this deep far more cheaply than malformed
/// ones, so the bound only has to sit below where backtracking becomes visible
/// (about 16 levels) and above anything hand-written.
const FLOW_DEPTH_GUARD: usize = 12;

/// Whether `text` opens more than [`FLOW_DEPTH_GUARD`] flow collections at once.
///
/// Deliberately blind to quoting and comments: a false positive only costs the
/// validation parse, a false negative would let a pathological document through.
fn exceeds_flow_depth(text: &str) -> bool {
    let mut depth = 0usize;
    for byte in text.bytes() {
        match byte {
            b'[' | b'{' => {
                depth += 1;
                if depth > FLOW_DEPTH_GUARD {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

/// YAML backend (validity lint + Prettier-compatible format via `oxc_formatter_yaml`).
pub struct YamlEngine;

static LANGUAGES: &[Language] = &[Language::Yaml];

impl Engine for YamlEngine {
    fn overridden_hooks(&self) -> u32 {
        poly_engine_api::desc::hooks::SKIP_REASON
    }

    fn name(&self) -> &'static str {
        "yaml"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: true,
            fix: false,
            project: false,
        }
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Format => OptionKeys::declared(&[
                ("line_width", OptionType::INTEGER),
                ("line_ending", OptionType::STRING),
                ("indent_style", OptionType::STRING),
                ("single_quote", OptionType::BOOLEAN),
                ("bracket_spacing", OptionType::BOOLEAN),
                ("trailing_commas", OptionType::STRING),
                ("prose_wrap", OptionType::STRING),
            ]),
            OptionTable::Lint | OptionTable::CrossCuttingLint => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        "0.1.0+serde_yaml-0.9.34+oxc_formatter_yaml-7053128+tmplskip+oxfmtrc-scope-1+tmpl-parse-2"
    }

    /// The path-scoped oxfmt config (`overrides`, nested configs) shapes the output and the cache key.
    fn host_hook(&self) -> poly_engine_api::host_hooks::HostHook {
        poly_engine_api::host_hooks::HostHook::OxfmtScope
    }

    fn skip_reason(&self, src: &SourceFile) -> Option<Cow<'static, str>> {
        is_helm_template(&src.content).then_some(Cow::Borrowed(GO_TEMPLATE_SKIP))
    }

    fn lint(&self, src: &SourceFile, _cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        if is_helm_template(&src.content) {
            return Ok(Vec::new());
        }
        match first_syntax_error(&src.content) {
            None => Ok(Vec::new()),
            Some(err) => {
                let (line, col) = err
                    .location()
                    .map_or((1, 1), |location| (location.line() as u32, location.column() as u32));
                Ok(vec![Diagnostic {
                    engine: "yaml".to_string(),
                    code: Some("syntax".to_string()),
                    severity: Severity::Error,
                    title: err.to_string(),
                    description: None,
                    url: None,
                    span: Some(Span {
                        start_line: line,
                        start_col: col,
                        end_line: line,
                        end_col: col,
                    }),
                    fix: vec![],
                    metadata: Default::default(),
                }])
            }
        }
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        if is_helm_template(&src.content) {
            return Ok(FormatOutput::Unchanged);
        }
        // Deeply nested unclosed flow collections make YAML parsers backtrack
        // exponentially, which wedges a rayon worker and with it the whole run.
        // A document `saphyr` rejects is one it would not format anyway, so deep
        // flow nesting is checked first.
        if exceeds_flow_depth(&src.content) && first_syntax_error(&src.content).is_some() {
            return Ok(FormatOutput::Unchanged);
        }
        let allocator = Allocator::new();
        let formatted = match oxc_formatter_yaml::format(&allocator, &src.content, build_format_options(cfg))
            .map_err(|_| ())
            .and_then(|formatted| formatted.print().map_err(|_| ()))
        {
            Ok(printed) => printed.into_code(),
            Err(()) => {
                tracing::debug!(path = %src.path.display(), "the YAML formatter declined the file");
                return Ok(FormatOutput::Unchanged);
            }
        };
        if formatted == *src.content {
            Ok(FormatOutput::Unchanged)
        } else {
            Ok(FormatOutput::Formatted(formatted))
        }
    }
}

/// Whether the document is Helm-style Go-templated YAML, which poly neither lints nor formats.
///
/// Template actions often *parse* as YAML flow mappings (`image: {{ .Values.image }}`), and the formatter
/// would rewrite them to `{ { .Values.image } }`, so a successful parse is no evidence the file is plain YAML.
/// A file is treated as ordinary YAML (Prometheus rules, Taskfiles) only when every `{{` is provably not
/// the start of a flow mapping; see [`only_inert_braces`].
fn is_helm_template(content: &str) -> bool {
    contains_go_template(content) && !only_inert_braces(content)
}

/// Whether every `{{` in `content` is inert text rather than the start of a flow mapping: inside a quoted
/// scalar, a block scalar or a comment, directly after a `$` (GitHub Actions), or inside a plain scalar that
/// starts with other text (`- echo {{.FOO}}`). Line-based and deliberately conservative: a `{{` at the start
/// of a line or of a value (after `:`, `-`, `?`, `[`, `{` or `,`) is not inert.
fn only_inert_braces(content: &str) -> bool {
    let mut quote: Option<u8> = None;
    let mut block_parent: Option<usize> = None;
    for line in content.lines() {
        let indent = line.len() - line.trim_start().len();
        if quote.is_none()
            && let Some(parent) = block_parent
        {
            if line.trim().is_empty() || indent > parent {
                continue;
            }
            block_parent = None;
        }
        let bytes = line.as_bytes();
        let mut end = line.len();
        let mut i = 0;
        while i < bytes.len() {
            let byte = bytes[i];
            if let Some(open) = quote {
                if open == b'"' && byte == b'\\' {
                    i += 2;
                    continue;
                }
                if byte == open {
                    if open == b'\'' && bytes.get(i + 1) == Some(&open) {
                        i += 2;
                        continue;
                    }
                    quote = None;
                }
            } else if byte == b'#' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) {
                end = i;
                break;
            } else if (byte == b'"' || byte == b'\'') && starts_value(&line[..i]) {
                quote = Some(byte);
            } else if byte == b'{' && bytes.get(i + 1) == Some(&b'{') {
                if !(i > 0 && bytes[i - 1] == b'$') && starts_value(&line[..i]) {
                    return false;
                }
                i += 2;
                continue;
            }
            i += 1;
        }
        if quote.is_none() {
            block_parent = block_scalar_parent(&line[..end], indent);
        }
    }
    true
}

/// Whether `before`, the text of a line ahead of some position, leaves that position at the start of a
/// value, key or flow entry.
fn starts_value(before: &str) -> bool {
    let before = before.trim_end();
    before.is_empty() || before.ends_with([':', '-', '?', '[', '{', ','])
}

/// The indent content must exceed to belong to the block scalar that `code` (a line without its comment)
/// opens, or `None` when it opens none.
fn block_scalar_parent(code: &str, indent: usize) -> Option<usize> {
    let code = code.trim_end();
    let (head, token) = code.rsplit_once(char::is_whitespace)?;
    let mut chars = token.chars();
    if !matches!(chars.next(), Some('|' | '>')) || !chars.all(|c| c.is_ascii_digit() || c == '+' || c == '-') {
        return None;
    }
    if !head.trim_end().ends_with([':', '-']) {
        return None;
    }
    // `- key: |` is parented by the key's column, not the dash's; a bare `- |` by the dash.
    let mut parent = indent;
    let mut rest = &code[indent..];
    while let Some(after) = rest.strip_prefix("- ") {
        let trimmed = after.trim_start();
        if trimmed == token {
            break;
        }
        parent += rest.len() - trimmed.len();
        rest = trimmed;
    }
    Some(parent)
}

/// The first scanner or parser error in any document of `content`.
fn first_syntax_error(content: &str) -> Option<serde_yaml::Error> {
    serde_yaml::Deserializer::from_str(content).find_map(|document| serde::de::IgnoredAny::deserialize(document).err())
}

/// Resolve [`YamlFormatOptions`] from the engine table, the shared layout options and the
/// oxfmt defaults for everything the table leaves unset.
pub(super) fn build_format_options(cfg: &EngineConfig) -> YamlFormatOptions {
    let mut options = YamlFormatOptions::default();
    options.apply_core(core_format_options(cfg));
    if let Some(single) = cfg.options.opt_bool("single_quote") {
        options.single_quote = single.into();
    }
    if let Some(spacing) = cfg.options.opt_bool("bracket_spacing") {
        options.bracket_spacing = spacing.into();
    }
    if cfg.options.opt_str("trailing_commas") == Some("never") {
        options.trailing_commas = TrailingCommas::Never;
    }
    options.prose_wrap = match cfg.options.opt_str("prose_wrap") {
        Some("always") => ProseWrap::Always,
        Some("never") => ProseWrap::Never,
        _ => ProseWrap::Preserve,
    };
    options
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use poly_engine_api::config::{EngineConfig, GlobalDefaults};

    fn default_cfg() -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::Table::new(),
        }
    }

    fn source(content: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from("templates/deployment.yaml"),
            language: Language::Yaml,
            content: content.into(),
        }
    }

    #[test]
    fn helm_templated_yaml_is_skipped_not_errored() {
        let engine = YamlEngine;
        // Go-templated YAML is not valid YAML; without the skip the scanner reports a syntax error.
        let src =
            source("metadata:\n  name: {{ .Release.Name }}\n  {{- if .Values.labels }}\n  labels: {}\n  {{- end }}\n");
        assert!(
            engine.lint(&src, &default_cfg()).expect("lint succeeded").is_empty(),
            "templated YAML must be skipped, not reported as a syntax error"
        );
        assert!(
            matches!(
                engine.format(&src, &default_cfg()).expect("format succeeded"),
                FormatOutput::Unchanged
            ),
            "templated YAML must be left unchanged by format"
        );
    }

    #[test]
    fn unclosed_nested_flow_collections_are_left_alone_instead_of_wedging_the_formatter() {
        let engine = YamlEngine;
        for opener in ["{", "["] {
            let src = source(&format!("a: 1\n{}x\n", opener.repeat(60)));
            assert!(
                matches!(
                    engine.format(&src, &default_cfg()).expect("format succeeded"),
                    FormatOutput::Unchanged
                ),
                "an invalid document must come back untouched"
            );
        }
    }

    #[test]
    fn deeply_nested_but_valid_flow_collections_are_still_formatted() {
        let engine = YamlEngine;
        let src = source(&format!("a:     {}1{}\n", "[".repeat(20), "]".repeat(20)));
        assert!(
            exceeds_flow_depth(&src.content),
            "the fixture must be deep enough to take the validation path"
        );
        assert!(
            matches!(
                engine.format(&src, &default_cfg()).expect("format succeeded"),
                FormatOutput::Formatted(_)
            ),
            "validity, not depth, decides whether a document is formatted"
        );
    }

    #[test]
    fn helm_actions_that_parse_as_flow_mappings_are_still_skipped() {
        let engine = YamlEngine;
        for content in [
            "name: {{ .Release.Name }}\n",
            "image: {{ .Values.image }}\n",
            "x: {{ include \"a\" . | nindent 4 }}\n",
            "list:\n  - {{ .Values.a }}\n",
            "{{- if .Values.x }}\nkind: ConfigMap\n{{- end }}\n",
        ] {
            let src = source(content);
            assert_eq!(
                engine.skip_reason(&src).as_deref(),
                Some(GO_TEMPLATE_SKIP),
                "{content:?}"
            );
            assert!(
                matches!(
                    engine.format(&src, &default_cfg()).expect("format succeeded"),
                    FormatOutput::Unchanged
                ),
                "{content:?} must stay untouched"
            );
        }
    }

    #[test]
    fn inert_template_text_is_not_skipped() {
        let engine = YamlEngine;
        for content in [
            "summary: \"{{ $labels.instance }} down\"\n",
            "summary: '{{ $labels.instance }} down'\n",
            "cmds:\n  - echo {{.FOO}}\n",
            "cmd: echo {{.FOO}}\n",
            "n: |\n  image: {{ .Values.image }}\nk: v\n",
            "- k: |\n    {{ .Values.a }}\n  o: 1\n",
            "# {{ .Values.a }}\nk: v\n",
            "k: v # {{ .Values.a }}\n",
            "a: \"x \\\" y\" # c\nb: \"multi\n  {{ .Values.a }}\"\n",
        ] {
            assert_eq!(engine.skip_reason(&source(content)).as_deref(), None, "{content:?}");
        }
    }

    #[test]
    fn a_block_scalar_ends_where_the_indent_returns() {
        assert!(!only_inert_braces("n: |\n  a\nk: {{ .Values.a }}\n"));
        assert!(!only_inert_braces("- k: |\n    a\n  o: {{ .Values.a }}\n"));
    }

    #[test]
    fn github_actions_expression_is_still_linted() {
        let engine = YamlEngine;
        // `${{ }}` is a valid YAML scalar and must NOT be skipped.
        let src = source("on: push\njobs:\n  build:\n    if: ${{ github.event_name == 'push' }}\n");
        assert!(
            engine.lint(&src, &default_cfg()).expect("lint succeeded").is_empty(),
            "valid GitHub Actions YAML should lint clean (and not be skipped as a template)"
        );
    }

    #[test]
    fn invalid_yaml_still_reports_syntax_error() {
        let engine = YamlEngine;
        let src = source("foo: [1, 2\nbar: :\n");
        let diags = engine.lint(&src, &default_cfg()).expect("lint ran");
        assert!(
            diags.iter().any(|d| d.code.as_deref() == Some("syntax")),
            "genuinely invalid YAML must still report a syntax error"
        );
    }
}
