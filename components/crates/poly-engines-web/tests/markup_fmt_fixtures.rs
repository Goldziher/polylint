//! Insta snapshot fixtures for the markup_fmt backend.
//!
//! markup_fmt is format-only (no diagnostics). One known-unformatted fixture per
//! language family asserts the exact canonical output:
//!
//! - `markup_fmt_known_unformatted_html`     — plain HTML
//! - `markup_fmt_known_unformatted_vue`      — Vue SFC template
//! - `markup_fmt_known_unformatted_svelte`   — Svelte component
//! - `markup_fmt_known_unformatted_astro`    — Astro component
//! - `markup_fmt_known_unformatted_angular`  — Angular component template (`*.component.html`)
//! - `markup_fmt_known_unformatted_jinja`    — Jinja2 / Twig / Nunjucks template
//! - `markup_fmt_known_unformatted_vento`    — Vento template
//! - `markup_fmt_known_unformatted_mustache` — Mustache / Handlebars template
//! - `markup_fmt_known_unformatted_xml`      — XML document

use poly_engine_api::{
    config::{EngineConfig, GlobalDefaults},
    declined::Declined,
    engine::{Engine, FormatOutput, SourceFile},
    language::Language,
};
use poly_engines_web::markup_fmt::MarkupFmtEngine;

fn engine_cfg() -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::Table::new(),
    }
}

fn make_src(path: &str, language: Language, content: &str) -> SourceFile {
    SourceFile {
        path: path.into(),
        language,
        content: content.into(),
    }
}

fn format_to_string(path: &str, language: Language, content: &str) -> String {
    let label = format!("{language:?}");
    let engine = MarkupFmtEngine;
    match engine
        .format(&make_src(path, language, content), &engine_cfg())
        .unwrap()
    {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => {
            panic!("fixture for {path} ({label}) was returned unchanged — it must be unformatted")
        }
    }
}

const KNOWN_UNFORMATTED_HTML: &str = "\
<html><head><title>Hi</title></head><body><p class=\"a\"   id=\"b\">Hello   world</p><div><span>x</span></div></body></html>";

#[test]
fn markup_fmt_known_unformatted_html() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_html",
        format_to_string("page.html", Language::Html, KNOWN_UNFORMATTED_HTML)
    );
}

const KNOWN_UNFORMATTED_VUE: &str = "\
<template><div class=\"box\"><Button   :label=\"title\" @click=\"onClick\">Go</Button></div></template>";

#[test]
fn markup_fmt_known_unformatted_vue() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_vue",
        format_to_string("App.vue", Language::Vue, KNOWN_UNFORMATTED_VUE)
    );
}

const KNOWN_UNFORMATTED_SVELTE: &str = "\
<script>let name = \"world\";</script><main><h1   class=\"title\">Hello {name}</h1><button on:click={go}>Go</button></main>";

#[test]
fn markup_fmt_known_unformatted_svelte() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_svelte",
        format_to_string("App.svelte", Language::Svelte, KNOWN_UNFORMATTED_SVELTE)
    );
}

const KNOWN_UNFORMATTED_ASTRO: &str = "\
---
const title = \"Hello\";
---
<html><head><title>{title}</title></head><body><p class=\"text\">Welcome</p></body></html>";

#[test]
fn markup_fmt_known_unformatted_astro() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_astro",
        format_to_string("index.astro", Language::Astro, KNOWN_UNFORMATTED_ASTRO)
    );
}

const NON_IDEMPOTENT_ASTRO_EXPRESSION: &str = r#"---
const renderedEntries = [];
---
{renderedEntries.length === 0 ? (
  <p>Empty</p>
) : (
  <Tabs>
    {renderedEntries.map(({ language, label, rendered }) => (
      <TabItem label={label} key={language}>
        <rendered.Content />
      </TabItem>
    ))}
  </Tabs>
)}
"#;

#[test]
fn markup_fmt_astro_expression_is_idempotent() {
    let once = format_to_string(
        "ApiSnippetGroup.astro",
        Language::Astro,
        NON_IDEMPOTENT_ASTRO_EXPRESSION,
    );
    let engine = MarkupFmtEngine;
    let twice = engine
        .format(
            &make_src("ApiSnippetGroup.astro", Language::Astro, &once),
            &engine_cfg(),
        )
        .unwrap();

    assert!(
        matches!(twice, FormatOutput::Unchanged),
        "formatting an Astro expression twice must reach a fixed point; got another rewrite"
    );
}

#[test]
fn markup_fmt_malformed_markup_is_left_unchanged() {
    let engine = MarkupFmtEngine;
    let output = engine
        .format(
            &make_src("legacy.html", Language::Html, "<html><body><div>legacy</body></html>"),
            &engine_cfg(),
        )
        .expect("malformed markup should be skipped without failing the repository run");

    assert!(matches!(output, FormatOutput::Unchanged));
}

#[test]
fn markup_fmt_astro_style_blocks_are_not_sent_to_js_formatter() {
    let engine = MarkupFmtEngine;
    let source =
        "---\nconst title = \"Hello\";\n---\n<style>.title{color:red}</style>\n<h1 class=\"title\">{title}</h1>\n";

    engine
        .format(&make_src("styles.astro", Language::Astro, source), &engine_cfg())
        .expect("Astro style blocks should not be parsed as JavaScript");
}

#[test]
fn markup_fmt_astro_invalid_embedded_javascript_is_left_unchanged() {
    let engine = MarkupFmtEngine;
    let source = "<script>const broken = ;</script>\n<main>legacy</main>\n";

    let output = engine
        .format(&make_src("legacy.astro", Language::Astro, source), &engine_cfg())
        .expect("invalid embedded JavaScript should not fail the repository format run");

    assert!(matches!(output, FormatOutput::Unchanged));
}

/// g-plane/markup_fmt#280: a regex holding a backtick inverts markup_fmt's template-literal tracker,
/// so it re-indents the content of the later template literal on every pass. The file is declined
/// and left as written rather than rewritten into a different runtime string.
const REGEX_BEFORE_MULTI_LINE_TEMPLATE: &str = "\
<html>
  <body>
    <script type=\"module\">
      const re = /`([^`]+)`/g;
      function f() {
        return `
  x
`;
      }
    </script>
  </body>
</html>
";

#[test]
fn markup_fmt_declines_a_script_whose_template_literal_it_would_rewrite() {
    let error = MarkupFmtEngine
        .format(
            &make_src("regex.html", Language::Html, REGEX_BEFORE_MULTI_LINE_TEMPLATE),
            &engine_cfg(),
        )
        .expect_err("the file must be declined, not rewritten");
    let reason = Declined::reason(&error).unwrap_or_else(|| panic!("expected a decline, got: {error:#}"));
    assert!(
        reason.contains("https://github.com/g-plane/markup_fmt/issues/280"),
        "the reason must name the upstream bug: {reason}"
    );
}

const MULTI_LINE_TEMPLATE: &str = "\
<script type=\"module\">
    const   b = `
  x
    y
`;
</script>
";

#[test]
fn markup_fmt_keeps_multi_line_template_content_and_settles() {
    let once = format_to_string("template.html", Language::Html, MULTI_LINE_TEMPLATE);
    assert!(once.contains("const b = `\n  x\n    y\n`;"), "{once}");
    let twice = MarkupFmtEngine
        .format(&make_src("template.html", Language::Html, &once), &engine_cfg())
        .unwrap();
    assert!(
        matches!(twice, FormatOutput::Unchanged),
        "formatting must reach a fixed point"
    );
}

const REGEX_WITH_BACKTICK: &str = "\
<script>
const   re = /`([^`]+)`/g;
const   a = `x`
</script>
";

#[test]
fn markup_fmt_formats_a_regex_with_a_backtick_and_no_multi_line_template() {
    let out = format_to_string("regex.html", Language::Html, REGEX_WITH_BACKTICK);
    assert!(out.contains("const re = /`([^`]+)`/g;"), "{out}");
    assert!(out.contains("const a = `x`;"), "{out}");
}

const KNOWN_UNFORMATTED_ANGULAR: &str = "\
<div class=\"container\"><p [class]=\"active ? 'on' : 'off'\">Hello</p><button (click)=\"doIt()\">Click</button></div>";

#[test]
fn markup_fmt_known_unformatted_angular() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_angular",
        format_to_string("app.component.html", Language::Angular, KNOWN_UNFORMATTED_ANGULAR,)
    );
}

const KNOWN_UNFORMATTED_JINJA: &str = "\
<html><head><title>{{ page_title }}</title></head><body><h1>{{ heading }}</h1><p class=\"intro\">{{ body }}</p></body></html>";

#[test]
fn markup_fmt_known_unformatted_jinja() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_jinja",
        format_to_string("base.html.jinja2", Language::Jinja, KNOWN_UNFORMATTED_JINJA)
    );
}

const KNOWN_UNFORMATTED_VENTO: &str = "\
<html><body><p>{{ title }}</p><ul>{{ for item of items }}<li>{{ item }}</li>{{ /for }}</ul></body></html>";

#[test]
fn markup_fmt_known_unformatted_vento() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_vento",
        format_to_string("layout.html.vto", Language::Vento, KNOWN_UNFORMATTED_VENTO)
    );
}

const KNOWN_UNFORMATTED_MUSTACHE: &str = "\
<html><head><title>{{title}}</title></head><body><p class=\"greeting\">{{message}}</p></body></html>";

#[test]
fn markup_fmt_known_unformatted_mustache() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_mustache",
        format_to_string("list.html.mustache", Language::Mustache, KNOWN_UNFORMATTED_MUSTACHE)
    );
}

const KNOWN_UNFORMATTED_XML: &str = "\
<?xml version=\"1.0\" encoding=\"UTF-8\"?><root><child attr=\"value\">text content</child><empty/></root>";

#[test]
fn markup_fmt_known_unformatted_xml() {
    insta::assert_snapshot!(
        "markup_fmt_known_unformatted_xml",
        format_to_string("config.xml", Language::Xml, KNOWN_UNFORMATTED_XML)
    );
}

/// A markup_fmt LanguageOptions field set via `[fmt.html.markup_fmt]` reaches
/// the formatter: `quotes = "single"` switches attribute quotes to single.
#[test]
fn markup_fmt_honors_language_option() {
    let engine = MarkupFmtEngine;
    let src = make_src("q.html", Language::Html, "<a href=\"x\">y</a>\n");
    let mut options = toml::Table::new();
    options.insert("quotes".to_string(), toml::Value::String("single".into()));
    let cfg = EngineConfig {
        options,
        ..engine_cfg()
    };
    let FormatOutput::Formatted(out) = engine.format(&src, &cfg).unwrap() else {
        panic!("`quotes = single` should switch attribute quotes");
    };
    assert!(
        out.contains("href='x'"),
        "[fmt.html.markup_fmt] quotes must reach markup_fmt; got: {out}"
    );
}
