//! Option parsing: what a malformed Go architecture entry reports.

use super::GoArch;

fn parse(calls: &str, imports: &str, layers: &str) -> anyhow::Result<Option<GoArch>> {
    let table = |text: &str| -> Vec<toml::Value> {
        let wrapped: toml::Table = toml::from_str(&format!("v = {text}")).expect("valid toml array");
        wrapped["v"].as_array().expect("an array").clone()
    };
    GoArch::from_values(Some(&table(calls)), Some(&table(imports)), Some(&table(layers)))
}

fn error(calls: &str, imports: &str, layers: &str) -> String {
    parse(calls, imports, layers)
        .expect_err("config must be rejected")
        .to_string()
}

#[test]
fn nothing_configured_is_no_arch() {
    assert!(GoArch::from_values(None, None, None).unwrap().is_none());
    assert!(parse("[]", "[]", "[]").unwrap().is_none());
}

#[test]
fn a_call_without_a_package_is_rejected_with_its_index() {
    assert_eq!(
        error(r#"[{ call = "os.Exit" }, { call = "Exit" }]"#, "[]", "[]"),
        "go_forbidden_calls[1]: `call` must be `<import path>.<Name>` (such as `os.Exit`), got `Exit`"
    );
}

#[test]
fn an_unknown_field_is_rejected_and_names_the_field() {
    let message = error(r#"[{ call = "os.Exit", allowed_in = ["cmd/**"] }]"#, "[]", "[]");
    assert!(
        message.starts_with("go_forbidden_calls[0]: unknown field `allowed_in`"),
        "{message}"
    );
}

#[test]
fn a_missing_required_field_is_rejected() {
    assert_eq!(
        error("[]", r#"[{ paths = ["a/**"] }]"#, "[]"),
        "go_forbidden_imports[0]: missing field `import`"
    );
}

#[test]
fn a_non_table_entry_is_rejected() {
    assert_eq!(
        error(r#"["os.Exit"]"#, "[]", "[]"),
        "go_forbidden_calls[0]: expected a table, found string"
    );
}

#[test]
fn an_invalid_glob_is_rejected() {
    let message = error("[]", "[]", r#"[{ name = "a", paths = ["internal/[a"] }]"#);
    assert!(
        message.starts_with("go_layers[0].paths: invalid glob `internal/[a`"),
        "{message}"
    );
}

#[test]
fn an_unknown_level_is_rejected() {
    assert_eq!(
        error(r#"[{ call = "os.Exit", level = "fatal" }]"#, "[]", "[]"),
        "go_forbidden_calls[0]: unknown level `fatal` (expected error, warning, info or hint)"
    );
}

#[test]
fn a_layer_may_import_only_declared_layers() {
    assert_eq!(
        error(
            "[]",
            "[]",
            r#"[{ name = "domain", paths = ["d/**"], may_import = ["infra"] }]"#
        ),
        "go_layers: layer `domain` may_import `infra`, which is not a declared layer"
    );
}

#[test]
fn layer_names_are_unique_and_paths_required() {
    assert_eq!(
        error(
            "[]",
            "[]",
            r#"[{ name = "a", paths = ["a/**"] }, { name = "a", paths = ["b/**"] }]"#
        ),
        "go_layers: layer `a` is declared twice"
    );
    assert_eq!(
        error("[]", "[]", r#"[{ name = "a", paths = [] }]"#),
        "go_layers[0]: layer `a` needs at least one entry in `paths`"
    );
}
