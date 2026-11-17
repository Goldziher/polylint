//! Markup backend: HTML / Vue / Svelte / Astro / Angular / Jinja / Vento /
//! Mustache / XML formatting via [`markup_fmt`].
//!
//! Capabilities: [`Capabilities::format`] only — markup_fmt is a formatter and
//! does not report diagnostics.
//!
//! ## Embedded code
//!
//! markup_fmt delegates embedded `<script>` / `<style>` blocks to an external
//! formatter callback. Astro JavaScript and TypeScript, and `<script>` in HTML, Vue and Svelte, are
//! routed through OXC; `<style>` blocks in HTML, Vue and Svelte through `oxc_formatter_css`. Vue and
//! Svelte template expressions reach the callback under the same extensions as scripts and an
//! expression is not a program, so [`is_script_block`] recognises a script block by its position in
//! the source and only those are formatted.
//!
//! markup_fmt re-indents a returned script body with a line-based quote tracker that a regex holding
//! a quote or backtick confuses, which rewrites the content of a later multi-line template literal
//! on every pass. A file whose output would change such a literal is declined instead
//! ([`template_guard`]; <https://github.com/g-plane/markup_fmt/issues/280>).
//!
//! ## Angular detection
//! Angular templates share the `.html` extension with plain HTML. poly
//! follows markup_fmt's own `detect_language` heuristic: a file whose stem
//! ends with `.component` (e.g. `app.component.html`) is routed to
//! `Language::Angular`; all other `.html` files go to `Language::Html`.
//!
//! ## Jinja covers Twig / Nunjucks
//! markup_fmt v0.27 exposes a single `Jinja` variant that handles Jinja2,
//! Twig, and Nunjucks templates. Extensions `.jinja`, `.jinja2`, `.j2`,
//! `.twig`, and `.njk` all route here.
//!
//! ## Mustache covers Handlebars
//! Similarly, `.mustache`, `.hbs`, and `.handlebars` all route to the
//! `Mustache` variant.
//!
//! ## Options layering
//! markup_fmt defaults → poly opinionated override (print_width 120,
//! indent_width 2 for all markup languages) → user
//! `[fmt.<lang>.markup_fmt]`.
//!
//! The user table is deserialized into [`markup_fmt::config::FormatOptions`]
//! (via the `config_serde` feature).  All
//! [`markup_fmt::config::LanguageOptions`] fields are exposed.  The layout
//! fields poly has a global for — `print_width` (`[defaults] line_length`) and
//! `line_break` (`[defaults] line_ending`) — default to that global and are
//! overridden when the user sets the key, per the documented layering.
//! `indent_width` comes from [`EngineConfig::indent_width`], itself resolved
//! from the same table's `indent_width` key.  `use_tabs` has no poly global and
//! is user-controlled.

use markup_fmt::Language as MarkupLanguage;
use markup_fmt::config::FormatOptions;
use markup_fmt::{Hints, format_text};
use std::borrow::Cow;

use poly_engine_api::config::EngineConfig;
use poly_engine_api::declined::Declined;
use poly_engine_api::engine::{Capabilities, Engine, FormatOutput, OptionKeys, OptionTable, SourceFile};
use poly_engine_api::language::Language;

mod template_guard;

/// markup_fmt HTML / Vue / Svelte / Astro / Angular / Jinja / Vento /
/// Mustache / XML formatter backend.
pub struct MarkupFmtEngine;

/// markup_fmt crate version — folded into the cache key so upgrades invalidate
/// any stale cached output.
/// Bumped suffix to +opts-1 after exposing full LanguageOptions (options were
/// previously ignored — existing caches must be invalidated).
const VERSION: &str = "0.27.6+opts-2+tmpltarget-2+embedded-oxc-3+rev:74e5e66+syntax-skip-2+script-fmt-2+tmpl-guard-1";

/// Reason reported when a general-purpose template does not render markup.
const NON_MARKUP_TEMPLATE_SKIP: &str = "template does not render markup";
/// Reason reported when a template does not name the language it renders.
const AMBIGUOUS_TEMPLATE_SKIP: &str = "ambiguous template target; add .html or .xml before the template extension";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TemplateTarget {
    Markup,
    NonMarkup,
    Ambiguous,
}

/// Languages handled by this backend.
static LANGUAGES: &[Language] = &[
    Language::Html,
    Language::Vue,
    Language::Svelte,
    Language::Astro,
    Language::Angular,
    Language::Jinja,
    Language::Vento,
    Language::Mustache,
    Language::Xml,
];

impl Engine for MarkupFmtEngine {
    fn overridden_hooks(&self) -> u32 {
        poly_engine_api::desc::hooks::SKIP_REASON
    }

    fn name(&self) -> &'static str {
        "markup_fmt"
    }

    fn languages(&self) -> &'static [Language] {
        LANGUAGES
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: false,
            format: true,
            fix: false,
            project: false,
        }
    }

    /// The whole table is deserialized into `markup_fmt::config::FormatOptions`,
    /// so the recognised keys are derived from that type instead of copied out
    /// of it — the upstream option set is large, versioned, and would drift.
    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        match table {
            OptionTable::Format => OptionKeys::declared(crate::oxc::JS_FORMAT_OPTION_KEYS)
                .with_derived(poly_engine_api::option_probe::recognized_by_type_probe::<markup_fmt::config::FormatOptions>)
                .with_note("Astro `<script>` blocks are formatted by the oxc backend from *this* table, so its format keys are read here too."),
            OptionTable::Lint | OptionTable::CrossCuttingLint => OptionKeys::declared(&[]),
        }
    }

    fn version(&self) -> &str {
        VERSION
    }

    fn skip_reason(&self, src: &SourceFile) -> Option<Cow<'static, str>> {
        if !is_generic_template(&src.language) {
            return None;
        }
        match template_target(&src.path) {
            TemplateTarget::Markup => None,
            TemplateTarget::NonMarkup => Some(Cow::Borrowed(NON_MARKUP_TEMPLATE_SKIP)),
            TemplateTarget::Ambiguous => Some(Cow::Borrowed(AMBIGUOUS_TEMPLATE_SKIP)),
        }
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        let Some(language) = markup_language(&src.language) else {
            return Ok(FormatOutput::Unchanged);
        };

        if self.skip_reason(src).is_some() {
            return Ok(FormatOutput::Unchanged);
        }

        let options = build_options(cfg);

        // Most files have no embedded block, so the clone is paid on the first one only.
        let mut scratch: Option<EngineConfig> = None;
        let mut templates = template_guard::ScriptTemplates::default();
        let formatted = format_text(&src.content, language, &options, |code, hints| {
            let embedded_scratch = scratch.get_or_insert_with(|| cfg.clone());
            embedded_scratch.globals.line_length = hints.print_width;
            let embedded_scratch = &*embedded_scratch;
            if let Some(ext) = script_ext(&src.language, &src.content, code, &hints) {
                let formatted = super::oxc::with_arena(|allocator| {
                    super::oxc::format_embedded_js(allocator, code, ext, embedded_scratch)
                });
                // Astro reports a script that does not parse; elsewhere it is left as written.
                let body = match formatted {
                    Ok(formatted) => Cow::Owned(formatted),
                    Err(error) if src.language == Language::Astro => return Err(anyhow::Error::new(error)),
                    Err(_) => Cow::Borrowed(code),
                };
                templates.record(&body, ext);
                return Ok(body);
            }
            if hints.attr
                || !matches!(src.language, Language::Html | Language::Vue | Language::Svelte)
                || !matches!(hints.ext, "css" | "scss" | "less")
            {
                return Ok(Cow::Borrowed(code));
            }
            let formatted = super::oxc::with_arena(|allocator| {
                let options = super::malva::oxc_css_options(embedded_scratch, css_variant(hints.ext));
                oxc_formatter_css::format(allocator, code, options)
                    .ok()
                    .and_then(|formatted| formatted.print().ok())
                    .map(|printed| printed.into_code())
            });
            Ok(formatted.map_or(Cow::Borrowed(code), Cow::Owned))
        });
        let formatted = match formatted {
            Ok(formatted) => formatted,
            Err(markup_fmt::FormatError::Syntax(_)) => return Ok(FormatOutput::Unchanged),
            Err(markup_fmt::FormatError::External(errors))
                if errors.iter().all(super::oxc::is_embedded_js_parse_error) =>
            {
                return Ok(FormatOutput::Unchanged);
            }
            Err(error) => return Err(error.into()),
        };

        if formatted == *src.content {
            return Ok(FormatOutput::Unchanged);
        }
        if !template_guard::preserved(&templates, &formatted, language, &options, &src.language) {
            return Err(anyhow::Error::new(Declined(template_guard::REASON.to_owned())));
        }
        Ok(FormatOutput::Formatted(formatted))
    }
}

fn css_variant(ext: &str) -> oxc_formatter_css::CssVariant {
    match ext {
        "scss" => oxc_formatter_css::CssVariant::Scss,
        "less" => oxc_formatter_css::CssVariant::Less,
        _ => oxc_formatter_css::CssVariant::Css,
    }
}

/// The extension `oxc_formatter` parses an embedded fragment as, or `None` when poly does not format
/// the fragment as a script.
///
/// Astro hands its frontmatter, scripts and expressions over under their own extensions. Vue and
/// Svelte route template expressions through the same callback with a `ts` extension, and an
/// expression is not a program (`{ a: 1 }` parses as a block), so only recognised script blocks
/// count there.
fn script_ext(language: &Language, source: &str, code: &str, hints: &Hints<'_>) -> Option<&'static str> {
    if *language == Language::Astro {
        return match hints.ext {
            "js" => Some("js"),
            "jsx" => Some("jsx"),
            "ts" => Some("ts"),
            "tsx" => Some("tsx"),
            _ => None,
        };
    }
    if hints.attr {
        return None;
    }
    match language {
        Language::Html => match hints.ext {
            "js" | "mjs" | "cjs" => Some("js"),
            "ts" | "mts" | "cts" => Some("ts"),
            "jsx" => Some("jsx"),
            "tsx" => Some("tsx"),
            _ => None,
        },
        Language::Vue | Language::Svelte if is_script_block(source, code) => match hints.ext {
            "js" | "mjs" => Some("js"),
            "ts" | "mts" => Some("ts"),
            "jsx" => Some("jsx"),
            "tsx" => Some("tsx"),
            _ => None,
        },
        _ => None,
    }
}

/// Whether `code` is the body of a `<script>` element of `source`, as opposed to a template
/// expression, binding or statement header that markup_fmt hands to the same callback.
///
/// markup_fmt passes the body as a slice of `source` and wraps every other fragment it sends in a
/// new string or cuts it from elsewhere in the template. The slice must therefore sit directly
/// between a `<script ...>` opening tag and `</script`.
fn is_script_block(source: &str, code: &str) -> bool {
    let Some(start) = (code.as_ptr() as usize).checked_sub(source.as_ptr() as usize) else {
        return false;
    };
    let end = start + code.len();
    if end > source.len() {
        return false;
    }
    let (Some(before), Some(after)) = (source.get(..start), source.get(end..)) else {
        return false;
    };
    let closes = after.get(..8).is_some_and(|tag| tag.eq_ignore_ascii_case("</script"));
    if !closes || !before.ends_with('>') {
        return false;
    }
    before
        .as_bytes()
        .windows(8)
        .rposition(|window| {
            window[..7].eq_ignore_ascii_case(b"<script") && (window[7].is_ascii_whitespace() || window[7] == b'>')
        })
        .is_some()
}

/// Whether this language is a *general-purpose* template syntax, i.e. one whose
/// rendered output is not necessarily markup.
///
/// Jinja, Vento and Mustache are routinely used to generate Go, Python, SQL and
/// other whitespace-sensitive source. `.html`/`.vue`/`.svelte`/`.astro`/`.xml`
/// are unambiguous and never need the check.
fn is_generic_template(language: &Language) -> bool {
    matches!(language, Language::Jinja | Language::Vento | Language::Mustache)
}

/// Classify the rendered language of a general-purpose template from its path.
///
/// markup_fmt reflows on the assumption that whitespace is insignificant, which
/// is true of HTML and false of most other languages. Applied to a template that
/// renders Go, it joined statements onto one line — turning
/// `data, err := json.Marshal(r)` followed by `if err != nil {` into
/// `json.Marshal(r) if err != nil` and emitting source that does not compile.
///
/// A double extension names the target directly (`marshal.go.jinja`). A bare
/// template is ambiguous: C# XML documentation, comparison operators, and
/// literal output can all look like markup without rendering markup.
///
/// Declining is the safe default here: leaving a template unformatted costs
/// nothing, while reflowing one destroys it.
fn template_target(path: &std::path::Path) -> TemplateTarget {
    if let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        && let Some((_, inner)) = stem.rsplit_once('.')
    {
        let inner = inner.to_ascii_lowercase();
        return if matches!(
            inner.as_str(),
            "html" | "htm" | "xhtml" | "xml" | "svg" | "vue" | "svelte" | "astro"
        ) {
            TemplateTarget::Markup
        } else {
            TemplateTarget::NonMarkup
        };
    }
    TemplateTarget::Ambiguous
}

/// Map a poly [`Language`] to the corresponding markup_fmt [`MarkupLanguage`].
fn markup_language(lang: &Language) -> Option<MarkupLanguage> {
    match lang {
        Language::Html => Some(MarkupLanguage::Html),
        Language::Vue => Some(MarkupLanguage::Vue),
        Language::Svelte => Some(MarkupLanguage::Svelte),
        Language::Astro => Some(MarkupLanguage::Astro),
        Language::Angular => Some(MarkupLanguage::Angular),
        Language::Jinja => Some(MarkupLanguage::Jinja),
        Language::Vento => Some(MarkupLanguage::Vento),
        Language::Mustache => Some(MarkupLanguage::Mustache),
        Language::Xml => Some(MarkupLanguage::Xml),
        _ => None,
    }
}

/// Build [`FormatOptions`] from a poly [`EngineConfig`].
///
/// Layering:
/// 1. `FormatOptions::default()` — markup_fmt's own defaults.
/// 2. If `cfg.options` is non-empty, deserialise into `FormatOptions` via
///    `config_serde`; unknown keys are silently ignored.
/// 3. Override all `LayoutOptions` fields with poly's globals — these always
///    win over any layout keys the user may have placed in the options table.
fn build_options(cfg: &EngineConfig) -> FormatOptions {
    let mut options: FormatOptions =
        poly_engine_api::rule_config::deserialize_options(cfg, "[fmt.<html|vue|svelte|…>.markup_fmt]");

    // The opinionated globals are a layer *under* the user's table, not over it
    // (ADR 0006 / ADR 0007): they apply only where the user did not set the key.
    if !poly_engine_api::rule_config::sets_any(cfg, &["print_width", "printWidth"]) {
        options.layout.print_width = cfg.globals.line_length;
    }
    options.layout.indent_width = cfg.indent_width;
    if !poly_engine_api::rule_config::sets_any(cfg, &["line_break", "lineBreak", "linebreak"]) {
        options.layout.line_break = match cfg.globals.line_ending {
            poly_engine_api::config::LineEnding::Crlf => markup_fmt::config::LineBreak::Crlf,
            poly_engine_api::config::LineEnding::Lf => markup_fmt::config::LineBreak::Lf,
        };
    }
    options
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use poly_engine_api::config::GlobalDefaults;

    fn engine_cfg() -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::Table::new(),
        }
    }

    fn make_src(path: &str, language: Language, content: &str) -> SourceFile {
        SourceFile {
            path: PathBuf::from(path),
            language,
            content: content.into(),
        }
    }

    /// A Jinja template rendering Go must be left alone. markup_fmt reflows on
    /// the assumption that whitespace is insignificant, which turned
    /// `data, err := json.Marshal(r)` + `if err != nil {` into
    /// `json.Marshal(r) if err != nil` — source that does not compile.
    #[test]
    fn template_rendering_non_markup_is_left_alone() {
        let go_template = concat!(
            "func (r *{{ type_name }}) MarshalC() ([]byte, error) {\n",
            "\tdata, err := json.Marshal(r)\n",
            "\tif err != nil {\n",
            "\t\treturn nil, err\n",
            "\t}\n",
            "\treturn data, nil\n",
            "}\n",
        );
        let src = make_src("marshal_receiver_to_c.jinja", Language::Jinja, go_template);

        assert!(matches!(
            MarkupFmtEngine.format(&src, &engine_cfg()).expect("format"),
            FormatOutput::Unchanged
        ));
    }

    /// A target-bearing extension opts a genuine HTML template into formatting.
    #[test]
    fn template_rendering_markup_is_still_formatted() {
        let src = make_src(
            "page.html.jinja",
            Language::Jinja,
            "<div    class=\"a\">\n<p>{{ name }}</p>\n</div>\n",
        );

        assert!(matches!(
            MarkupFmtEngine.format(&src, &engine_cfg()).expect("format"),
            FormatOutput::Formatted(_)
        ));
    }

    /// Content sniffing cannot distinguish C# XML documentation from rendered
    /// XML. A bare template therefore stays byte-for-byte intact and explains
    /// how to opt into markup formatting.
    #[test]
    fn ambiguous_bare_template_with_markup_like_content_is_skipped() {
        let content = concat!(
            "{% if method.has_docs %}\n",
            "/// <summary>\n",
            "/// {{ method.docs }}\n",
            "/// </summary>\n",
            "{% endif %}\n",
            "public void {{ method.name }}() {}\n",
        );
        let src = make_src("service_method.jinja", Language::Jinja, content);

        assert_eq!(
            MarkupFmtEngine.skip_reason(&src).as_deref(),
            Some("ambiguous template target; add .html or .xml before the template extension")
        );
        assert!(matches!(
            MarkupFmtEngine.format(&src, &engine_cfg()).expect("format"),
            FormatOutput::Unchanged
        ));
        assert_eq!(src.content.as_ref(), content, "skipping must preserve every byte");
    }

    /// A double extension names the target outright and outranks content.
    #[test]
    fn double_extension_decides_the_target() {
        let go = make_src("tmpl.go.jinja", Language::Jinja, "func F() {\n\tx := 1\n}\n");
        assert!(matches!(
            MarkupFmtEngine.format(&go, &engine_cfg()).expect("format"),
            FormatOutput::Unchanged
        ));

        let html = make_src("page.html.jinja", Language::Jinja, "<div    class=\"a\">\n</div>\n");
        assert!(matches!(
            MarkupFmtEngine.format(&html, &engine_cfg()).expect("format"),
            FormatOutput::Formatted(_)
        ));
    }

    /// Bare templates are ambiguous regardless of markup-looking content.
    #[test]
    fn bare_template_content_does_not_override_ambiguous_target() {
        assert_eq!(
            template_target(std::path::Path::new("service.jinja")),
            TemplateTarget::Ambiguous
        );
        assert_eq!(
            template_target(std::path::Path::new("service.xml.jinja")),
            TemplateTarget::Markup
        );
        assert_eq!(
            template_target(std::path::Path::new("service.cs.jinja")),
            TemplateTarget::NonMarkup
        );
    }

    #[test]
    fn every_generic_template_language_requires_a_rendered_target() {
        for (path, language) in [
            ("page.jinja", Language::Jinja),
            ("page.vto", Language::Vento),
            ("page.mustache", Language::Mustache),
        ] {
            let src = make_src(path, language, "<summary>{{ docs }}</summary>\n");
            assert_eq!(
                MarkupFmtEngine.skip_reason(&src).as_deref(),
                Some(AMBIGUOUS_TEMPLATE_SKIP)
            );
        }
    }

    /// Unambiguous markup languages skip the check entirely.
    #[test]
    fn dedicated_markup_languages_are_not_sniffed() {
        for language in [Language::Html, Language::Vue, Language::Svelte, Language::Xml] {
            assert!(!is_generic_template(&language), "{language:?} needs no target check");
        }
        for language in [Language::Jinja, Language::Vento, Language::Mustache] {
            assert!(is_generic_template(&language), "{language:?} must be checked");
        }
    }

    #[test]
    fn engine_metadata() {
        let engine = MarkupFmtEngine;
        assert_eq!(engine.name(), "markup_fmt");
        for lang in &[
            Language::Html,
            Language::Vue,
            Language::Svelte,
            Language::Astro,
            Language::Angular,
            Language::Jinja,
            Language::Vento,
            Language::Mustache,
            Language::Xml,
        ] {
            assert!(
                engine.languages().contains(lang),
                "{lang:?} should be listed in MarkupFmtEngine::languages()"
            );
        }
        let caps = engine.capabilities();
        assert!(!caps.lint);
        assert!(caps.format);
        assert!(!caps.fix);
    }

    fn formatted_text(language: Language, path: &str, content: &str) -> String {
        match MarkupFmtEngine
            .format(&make_src(path, language, content), &engine_cfg())
            .expect("format")
        {
            FormatOutput::Formatted(text) => text,
            FormatOutput::Unchanged => content.to_string(),
        }
    }

    #[test]
    fn html_style_and_script_blocks_are_formatted() {
        let out = formatted_text(
            Language::Html,
            "a.html",
            "<style>\n.a{color:red}\n</style>\n<script>\nconst   a=1\n</script>\n",
        );
        assert!(out.contains("color: red;"), "{out}");
        assert!(out.contains("const a = 1;"), "{out}");
    }

    #[test]
    fn html_module_script_is_formatted_and_settles() {
        let input = "<script type=\"module\">\n    import a from \"x\";\n    const   b = `\n  x\n`;\n    const c = {\n      d: 1 };\n</script>\n";
        let once = formatted_text(Language::Html, "a.html", input);
        assert!(once.contains("const b = `"), "{once}");
        assert_eq!(formatted_text(Language::Html, "a.html", &once), once);
    }

    #[test]
    fn vue_style_and_script_are_formatted_and_expressions_are_left_alone() {
        let input = "<template>\n  <p :a=\"{ b: 1 }\">{{ x }}</p>\n</template>\n<script setup lang=\"ts\">\nconst   a:number=1\n</script>\n<style>\n.a{color:red}\n</style>\n";
        let out = formatted_text(Language::Vue, "a.vue", input);
        assert!(out.contains("color: red;"), "{out}");
        assert!(out.contains("const a: number = 1;"), "{out}");
        assert!(out.contains(":a=\"{ b: 1 }\""), "{out}");
    }

    #[test]
    fn svelte_script_is_formatted_and_expressions_are_left_alone() {
        let input = "<script lang=\"ts\">\nlet   n:number=1\n</script>\n<p class={ {a:1}.a }>{n}</p>\n";
        let out = formatted_text(Language::Svelte, "a.svelte", input);
        assert!(out.contains("let n: number = 1;"), "{out}");
    }

    #[test]
    fn script_block_detection_rejects_expressions() {
        let source = "<template><p>{{ x }}</p></template>\n<script setup generics=\"T extends Array<string>\">\nlet a\n</script>";
        let body = &source[source.find("\nlet").unwrap()..source.find("</script").unwrap()];
        assert!(is_script_block(source, body));
        let expr = &source[source.find("x }}").unwrap()..][..1];
        assert!(!is_script_block(source, expr));
        assert!(!is_script_block(source, "\nlet a\n"));
    }

    #[test]
    fn unsupported_language_is_unchanged() {
        let engine = MarkupFmtEngine;
        let src = make_src("x.txt", Language::Other("text".into()), "hello\n");
        assert!(matches!(
            engine.format(&src, &engine_cfg()).unwrap(),
            FormatOutput::Unchanged
        ));
    }
}
