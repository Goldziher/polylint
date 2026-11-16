//! Fixtures for the JSON Schema engine: a valid and an invalid document per format, every schema
//! source, the skip diagnostics and the opt-in gate. Expected positions are derived by hand from
//! the fixture text; each is `(rule, line, column)`.

use std::path::Path;

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, PlanPolicy, Severity, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_jsonschema::JsonSchemaEngine;

const SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "required": ["name", "port"],
  "properties": {
    "$schema": { "type": "string" },
    "name": { "type": "string" },
    "port": { "type": "integer" },
    "mode": { "enum": ["fast", "slow"] }
  },
  "additionalProperties": false
}
"#;

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("poly.toml"), "").expect("poly.toml");
    std::fs::write(dir.path().join("schema.json"), SCHEMA).expect("schema.json");
    dir
}

fn write(dir: &Path, name: &str, content: &str) {
    let path = dir.join(name);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    std::fs::write(path, content).expect("write");
}

fn config(extra: &str) -> EngineConfig {
    let options: toml::Table = toml::from_str(&format!("enabled = true\n{extra}")).expect("options");
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options,
    }
}

fn lint_with(dir: &Path, name: &str, language: Language, content: &str, cfg: &EngineConfig) -> Vec<Diagnostic> {
    write(dir, name, content);
    let src = SourceFile {
        path: dir.join(name),
        language,
        content: content.into(),
    };
    JsonSchemaEngine.lint(&src, cfg).expect("lint")
}

fn tuples(diagnostics: &[Diagnostic]) -> Vec<(String, u32, u32)> {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("span");
            (d.code.clone().expect("code"), span.start_line, span.start_col)
        })
        .collect()
}

fn lint(dir: &Path, name: &str, language: Language, content: &str) -> Vec<(String, u32, u32)> {
    tuples(&lint_with(dir, name, language, content, &config("")))
}

fn expect(rules: &[(&str, u32, u32)]) -> Vec<(String, u32, u32)> {
    rules
        .iter()
        .map(|(rule, line, col)| ((*rule).to_owned(), *line, *col))
        .collect()
}

const JSON_BAD: &str =
    "{\n  \"$schema\": \"./schema.json\",\n  \"name\": \"app\",\n  \"port\": \"80\",\n  \"mode\": \"turbo\"\n}\n";
const JSON_GOOD: &str =
    "{\n  \"$schema\": \"./schema.json\",\n  \"name\": \"app\",\n  \"port\": 80,\n  \"mode\": \"fast\"\n}\n";
const YAML_BAD: &str = "# yaml-language-server: $schema=./schema.json\nname: app\nport: \"80\"\nmode: turbo\n";
const YAML_GOOD: &str = "# yaml-language-server: $schema=./schema.json\nname: app\nport: 80\nmode: fast\n";
const TOML_BAD: &str = "#:schema ./schema.json\nname = \"app\"\nport = \"80\"\nmode = \"turbo\"\n";
const TOML_GOOD: &str = "#:schema ./schema.json\nname = \"app\"\nport = 80\nmode = \"fast\"\n";

#[test]
fn json_names_its_schema_by_a_relative_dollar_schema() {
    let dir = project();
    assert_eq!(
        lint(dir.path(), "app.json", Language::Json, JSON_BAD),
        expect(&[("schema/type", 4, 11), ("schema/enum", 5, 11)])
    );
    assert!(lint(dir.path(), "app.json", Language::Json, JSON_GOOD).is_empty());
}

#[test]
fn jsonc_tolerates_comments_and_trailing_commas() {
    let dir = project();
    let content = "// service config\n{\n  \"$schema\": \"./schema.json\", /* local */\n  \"name\": \"app\",\n  \"port\": \"80\",\n}\n";
    assert_eq!(
        lint(dir.path(), "app.jsonc", Language::Jsonc, content),
        expect(&[("schema/type", 5, 11)])
    );
}

#[test]
fn yaml_names_its_schema_by_a_language_server_comment() {
    let dir = project();
    assert_eq!(
        lint(dir.path(), "app.yaml", Language::Yaml, YAML_BAD),
        expect(&[("schema/type", 3, 7), ("schema/enum", 4, 7)])
    );
    assert!(lint(dir.path(), "app.yaml", Language::Yaml, YAML_GOOD).is_empty());
}

#[test]
fn toml_names_its_schema_by_a_schema_directive() {
    let dir = project();
    assert_eq!(
        lint(dir.path(), "app.toml", Language::Toml, TOML_BAD),
        expect(&[("schema/type", 3, 8), ("schema/enum", 4, 8)])
    );
    assert!(lint(dir.path(), "app.toml", Language::Toml, TOML_GOOD).is_empty());
}

#[test]
fn a_missing_required_property_is_reported_at_the_object() {
    let dir = project();
    let content = "{\n  \"$schema\": \"./schema.json\",\n  \"name\": \"app\"\n}\n";
    assert_eq!(
        lint(dir.path(), "app.json", Language::Json, content),
        expect(&[("schema/required", 1, 1)])
    );
}

#[test]
fn a_glob_mapping_names_the_schema_of_a_file_that_names_none() {
    let dir = project();
    let cfg = config("schemas = [{ glob = \"config/*.yaml\", schema = \"schema.json\" }]\n");
    let content = "name: app\nport: \"80\"\n";
    let mapped = tuples(&lint_with(dir.path(), "config/app.yaml", Language::Yaml, content, &cfg));
    assert_eq!(mapped, expect(&[("schema/type", 2, 7)]));
    let unmapped = tuples(&lint_with(dir.path(), "other/app.yaml", Language::Yaml, content, &cfg));
    assert!(
        unmapped.is_empty(),
        "a file no glob matches names no schema: {unmapped:?}"
    );
}

#[test]
fn a_ref_to_a_sibling_schema_file_resolves_locally() {
    let dir = project();
    write(
        dir.path(),
        "schemas/main.json",
        r##"{ "type": "object", "properties": { "port": { "$ref": "defs.json#/$defs/port" } } }"##,
    );
    write(
        dir.path(),
        "schemas/defs.json",
        r#"{ "$defs": { "port": { "type": "integer" } } }"#,
    );
    let cfg = config("schemas = [{ glob = \"*.yaml\", schema = \"schemas/main.json\" }]\n");
    let found = tuples(&lint_with(dir.path(), "a.yaml", Language::Yaml, "port: \"80\"\n", &cfg));
    assert_eq!(found, expect(&[("schema/type", 1, 7)]));
}

#[test]
fn an_uncached_remote_schema_is_a_visible_info_skip() {
    let dir = project();
    let content = "{\n  \"$schema\": \"https://example.com/schemas/app.json\",\n  \"name\": \"app\"\n}\n";
    let found = lint_with(dir.path(), "app.json", Language::Json, content, &config(""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_deref(), Some("schema-unavailable"));
    assert_eq!(found[0].severity, Severity::Info);
    assert!(
        found[0].title.contains("https://example.com/schemas/app.json"),
        "{}",
        found[0].title
    );
}

#[test]
fn a_cached_remote_schema_validates_without_the_network() {
    let dir = project();
    write(dir.path(), ".poly/schemas/example.com/schemas/app.json", SCHEMA);
    let content = "{\n  \"$schema\": \"https://example.com/schemas/app.json\",\n  \"name\": \"app\"\n}\n";
    assert_eq!(
        lint(dir.path(), "app.json", Language::Json, content),
        expect(&[("schema/required", 1, 1)])
    );
}

#[test]
fn a_meta_schema_reference_marks_a_schema_file_and_names_nothing() {
    let dir = project();
    assert!(lint(dir.path(), "schema.json", Language::Json, SCHEMA).is_empty());
}

#[test]
fn a_schema_outside_the_project_root_is_unavailable() {
    let dir = project();
    let content = "{\n  \"$schema\": \"../elsewhere.json\",\n  \"name\": \"app\"\n}\n";
    let found = lint_with(dir.path(), "app.json", Language::Json, content, &config(""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_deref(), Some("schema-unavailable"));
}

#[test]
fn a_schema_that_is_not_json_is_an_error() {
    let dir = project();
    write(dir.path(), "broken.json", "{ not json");
    let content = "#:schema ./broken.json\nname = \"app\"\n";
    let found = lint_with(dir.path(), "app.toml", Language::Toml, content, &config(""));
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_deref(), Some("schema-invalid"));
    assert_eq!(found[0].severity, Severity::Error);
}

#[test]
fn a_yaml_schema_validates_documents() {
    let dir = project();
    write(
        dir.path(),
        "schema.yaml",
        "type: object\nrequired: [name]\nproperties:\n  name: {type: string}\n",
    );
    let bad = "# yaml-language-server: $schema=./schema.yaml\nname: 3\n";
    assert_eq!(
        lint(dir.path(), "app.yaml", Language::Yaml, bad),
        expect(&[("schema/type", 2, 7)])
    );
    let good = "# yaml-language-server: $schema=./schema.yaml\nname: app\n";
    assert!(lint(dir.path(), "app.yaml", Language::Yaml, good).is_empty());
}

#[test]
fn a_yaml_schema_may_use_merge_keys() {
    let dir = project();
    write(
        dir.path(),
        "schema.yml",
        "base: &b {type: string}\ntype: object\nproperties:\n  name:\n    <<: *b\n    minLength: 1\n",
    );
    let bad = "# yaml-language-server: $schema=./schema.yml\nname: 3\n";
    assert_eq!(
        lint(dir.path(), "app.yaml", Language::Yaml, bad),
        expect(&[("schema/type", 2, 7)])
    );
}

#[test]
fn yaml_merge_keys_satisfy_required_and_additional_properties() {
    let dir = project();
    write(
        dir.path(),
        "compose.json",
        r#"{"type":"object","properties":{"x":{"type":"object"},"svc":{"type":"object","required":["name"],"properties":{"name":{"type":"string"}},"additionalProperties":false}}}"#,
    );
    let content = "# yaml-language-server: $schema=./compose.json\nx: &d {name: a}\nsvc: {<<: *d}\n";
    assert!(lint(dir.path(), "c.yaml", Language::Yaml, content).is_empty());
}

#[test]
fn a_document_that_does_not_parse_is_an_info_skip() {
    let dir = project();
    let found = lint_with(
        dir.path(),
        "app.toml",
        Language::Toml,
        "#:schema ./schema.json\nname = \n",
        &config(""),
    );
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].code.as_deref(), Some("schema-unparseable"));
    assert_eq!(found[0].severity, Severity::Info);
}

#[test]
fn a_file_naming_no_schema_is_not_touched() {
    let dir = project();
    assert!(lint(dir.path(), "plain.yaml", Language::Yaml, "name: app\nport: \"80\"\n").is_empty());
}

#[test]
fn the_engine_is_off_until_enabled() {
    let dir = project();
    let off = EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 2,
        options: toml::Table::new(),
    };
    assert!(lint_with(dir.path(), "app.json", Language::Json, JSON_BAD, &off).is_empty());
}

#[test]
fn every_stream_document_of_a_yaml_file_is_validated() {
    let dir = project();
    let content = "# yaml-language-server: $schema=./schema.json\nname: a\nport: 1\n---\nname: b\nport: \"2\"\n";
    assert_eq!(
        lint(dir.path(), "multi.yaml", Language::Yaml, content),
        expect(&[("schema/type", 6, 7)])
    );
}

#[test]
fn results_are_identical_across_runs() {
    let dir = project();
    let first = lint_with(dir.path(), "app.json", Language::Json, JSON_BAD, &config(""));
    let second = lint_with(dir.path(), "app.json", Language::Json, JSON_BAD, &config(""));
    let titles = |found: &[Diagnostic]| found.iter().map(|d| (d.title.clone(), d.span)).collect::<Vec<_>>();
    assert_eq!(titles(&first), titles(&second));
    assert!(
        first
            .iter()
            .all(|d| d.severity == Severity::Error && d.engine == "jsonschema")
    );
}

#[test]
fn the_engine_claims_no_language_coverage() {
    let cfg = config("");
    for language in [Language::Json, Language::Jsonc, Language::Yaml, Language::Toml] {
        assert!(
            !JsonSchemaEngine.provides_language_lint(&language, &cfg),
            "{language:?}"
        );
    }
    assert!(JsonSchemaEngine.capabilities().lint && !JsonSchemaEngine.capabilities().format);
}

#[test]
fn the_family_registers_only_its_languages() {
    use poly_engines_jsonschema::engines_for;
    assert_eq!(engines_for(&Language::Yaml).len(), 1);
    assert!(engines_for(&Language::Python).is_empty());
}
