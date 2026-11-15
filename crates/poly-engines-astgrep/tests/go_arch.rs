//! Fixtures for the configurable Go architecture rules: forbidden calls, forbidden imports and
//! import layering, each linted through the real `AstGrepEngine` against files on disk so the
//! `go.mod` lookup is exercised.

use std::fs;

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, Severity, SourceFile};
use poly_engine_api::host_hooks::HostHook;
use poly_engine_api::language::Language;
use poly_engines_astgrep::AstGrepEngine;

fn repo(files: &[(&str, &str)]) -> tempfile::TempDir {
    let repo = tempfile::tempdir().unwrap();
    for (name, text) in files {
        let path = repo.path().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    repo
}

fn config(options: &str) -> EngineConfig {
    let mut options: toml::Table = toml::from_str(options).expect("valid options");
    options.insert("builtin_pack_enabled".to_owned(), toml::Value::Boolean(false));
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options,
    }
}

fn lint_result(repo: &tempfile::TempDir, file: &str, options: &str) -> anyhow::Result<Vec<Diagnostic>> {
    let path = repo.path().join(file);
    let content = fs::read_to_string(&path).unwrap();
    let src = SourceFile {
        path,
        language: Language::Go,
        content: content.into(),
    };
    AstGrepEngine.lint(&src, &config(options))
}

/// `code@line: title` per finding, in report order.
fn lint(repo: &tempfile::TempDir, file: &str, options: &str) -> Vec<String> {
    lint_result(repo, file, options)
        .unwrap()
        .iter()
        .map(|d| {
            format!(
                "{}@{}: {}",
                d.code.as_deref().unwrap(),
                d.span.as_ref().unwrap().start_line,
                d.title
            )
        })
        .collect()
}

const EXIT: &str = "package x\n\nimport \"os\"\n\nfunc f() {\n\tos.Exit(1)\n}\n";

#[test]
fn a_forbidden_call_is_flagged_at_the_selector() {
    let repo = repo(&[("go.mod", "module example.com/app\n"), ("internal/x/x.go", EXIT)]);
    assert_eq!(
        lint(
            &repo,
            "internal/x/x.go",
            r#"go_forbidden_calls = [{ call = "os.Exit" }]"#
        ),
        ["go-forbidden-call@6: `os.Exit` is forbidden here (matches `os.Exit`)"]
    );
}

#[test]
fn a_clean_file_and_an_unconfigured_run_report_nothing() {
    let repo = repo(&[("go.mod", "module example.com/app\n"), ("internal/x/x.go", EXIT)]);
    assert!(
        lint(
            &repo,
            "internal/x/x.go",
            r#"go_forbidden_calls = [{ call = "os.Getenv" }]"#
        )
        .is_empty()
    );
    assert!(lint(&repo, "internal/x/x.go", "").is_empty());
}

#[test]
fn allow_in_globs_are_relative_to_the_module_root() {
    let repo = repo(&[
        ("go.mod", "module example.com/app\n"),
        ("main.go", EXIT),
        ("cmd/app/main.go", EXIT),
        ("internal/x/x.go", EXIT),
        ("tools/main.go", EXIT),
    ]);
    let options = r#"go_forbidden_calls = [{ call = "os.Exit", allow_in = ["cmd/**", "main.go"] }]"#;
    assert!(lint(&repo, "main.go", options).is_empty());
    assert!(lint(&repo, "cmd/app/main.go", options).is_empty());
    assert_eq!(lint(&repo, "internal/x/x.go", options).len(), 1);
    assert_eq!(
        lint(&repo, "tools/main.go", options).len(),
        1,
        "`main.go` is the root file only"
    );
}

#[test]
fn paths_scope_a_rule_to_part_of_the_module() {
    let repo = repo(&[
        ("go.mod", "module example.com/app\n"),
        ("internal/x/x.go", EXIT),
        ("cmd/app/main.go", EXIT),
    ]);
    let options = r#"go_forbidden_calls = [{ call = "os.Exit", paths = ["internal/**"] }]"#;
    assert_eq!(lint(&repo, "internal/x/x.go", options).len(), 1);
    assert!(lint(&repo, "cmd/app/main.go", options).is_empty());
}

#[test]
fn the_selector_name_takes_a_wildcard_and_aliases_are_followed() {
    let source = "package x\n\nimport (\n\tl \"log\"\n\t\"log\"\n)\n\nfunc f() {\n\tl.Fatal(\"a\")\n\tlog.Fatalf(\"b\")\n\tlog.Printf(\"c\")\n}\n";
    let repo = repo(&[("go.mod", "module m\n"), ("x.go", source)]);
    assert_eq!(
        lint(&repo, "x.go", r#"go_forbidden_calls = [{ call = "log.Fatal*" }]"#),
        [
            "go-forbidden-call@9: `l.Fatal` is forbidden here (matches `log.Fatal*`)",
            "go-forbidden-call@10: `log.Fatalf` is forbidden here (matches `log.Fatal*`)",
        ]
    );
}

#[test]
fn the_package_is_an_import_path_and_a_version_suffix_names_the_package_before_it() {
    let source = "package x\n\nimport (\n\t\"gopkg.in/yaml.v3\"\n\t\"os/exec\"\n\t\"example.com/lib/v2\"\n)\n\nfunc f() {\n\t_ = yaml.Unmarshal\n\t_ = exec.Command(\"ls\")\n\t_ = lib.Do()\n}\n";
    let repo = repo(&[("go.mod", "module m\n"), ("x.go", source)]);
    let options = r#"go_forbidden_calls = [
        { call = "gopkg.in/yaml.v3.Unmarshal", message = "use the typed decoder" },
        { call = "os/exec.*" },
        { call = "example.com/lib/v2.Do", level = "warning" },
    ]"#;
    assert_eq!(
        lint(&repo, "x.go", options),
        [
            "go-forbidden-call@10: use the typed decoder",
            "go-forbidden-call@11: `exec.Command` is forbidden here (matches `os/exec.*`)",
            "go-forbidden-call@12: `lib.Do` is forbidden here (matches `example.com/lib/v2.Do`)",
        ]
    );
}

#[test]
fn a_selector_on_something_that_is_not_the_imported_package_is_left_alone() {
    let source = "package x\n\nimport \"os\"\n\nfunc f(cfg Config) {\n\t_ = cfg.Exit\n\t_ = os.Args\n}\n";
    let repo = repo(&[("go.mod", "module m\n"), ("x.go", source)]);
    assert!(lint(&repo, "x.go", r#"go_forbidden_calls = [{ call = "os.Exit" }]"#).is_empty());
}

#[test]
fn a_dot_import_of_a_forbidden_package_is_the_finding() {
    let source = "package x\n\nimport . \"os\"\n\nfunc f() {\n\tExit(1)\n}\n";
    let repo = repo(&[("go.mod", "module m\n"), ("x.go", source)]);
    assert_eq!(
        lint(&repo, "x.go", r#"go_forbidden_calls = [{ call = "os.Exit" }]"#),
        ["go-forbidden-call@3: dot import of `os` hides the selectors forbidden by `os.Exit`; import it by name"]
    );
}

#[test]
fn test_files_are_skipped_unless_a_rule_asks_for_them() {
    let repo = repo(&[("go.mod", "module m\n"), ("x_test.go", EXIT)]);
    assert!(lint(&repo, "x_test.go", r#"go_forbidden_calls = [{ call = "os.Exit" }]"#).is_empty());
    assert_eq!(
        lint(
            &repo,
            "x_test.go",
            r#"go_forbidden_calls = [{ call = "os.Exit", include_tests = true }]"#
        )
        .len(),
        1
    );
}

#[test]
fn levels_follow_the_entry_then_the_rule_override_and_select_ignore_apply() {
    let repo = repo(&[("go.mod", "module m\n"), ("x.go", EXIT)]);
    let severity = |options: &str| lint_result(&repo, "x.go", options).unwrap().first().map(|d| d.severity);
    let call = r#"go_forbidden_calls = [{ call = "os.Exit", level = "info" }]"#;
    assert_eq!(severity(call), Some(Severity::Info));
    assert_eq!(
        severity(r#"go_forbidden_calls = [{ call = "os.Exit" }]"#),
        Some(Severity::Error)
    );
    assert_eq!(
        severity(&format!("{call}\n[rules.go-forbidden-call]\nlevel = \"warning\"")),
        Some(Severity::Warning)
    );
    assert_eq!(severity(&format!("ignore = [\"go-forbidden-call\"]\n{call}")), None);
    assert_eq!(severity(&format!("select = [\"other\"]\n{call}")), None);
    assert_eq!(
        severity(&format!(
            "select = [\"other\"]\nextend_select = [\"go-forbidden-call\"]\n{call}"
        )),
        Some(Severity::Info)
    );
}

#[test]
fn a_forbidden_import_is_flagged_within_its_paths_only() {
    let source = "package x\n\nimport (\n\t\"fmt\"\n\t\"github.com/sigstore/cosign/v2/pkg\"\n)\n";
    let repo = repo(&[("go.mod", "module m\n"), ("pkg/a/a.go", source), ("cmd/c/c.go", source)]);
    let options = r#"go_forbidden_imports = [{ import = "github.com/sigstore/**", paths = ["pkg/**"], message = "keep sigstore out of the library" }]"#;
    assert_eq!(
        lint(&repo, "pkg/a/a.go", options),
        ["go-forbidden-import@5: keep sigstore out of the library"]
    );
    assert!(lint(&repo, "cmd/c/c.go", options).is_empty());
}

const LAYERS: &str = r#"
go_layers = [
    { name = "domain", paths = ["internal/domain/**"], may_import = [] },
    { name = "app", paths = ["internal/app/**"], may_import = ["domain"] },
    { name = "infra", paths = ["internal/infra/**"], may_import = ["domain", "app"] },
]
"#;

#[test]
fn a_layer_may_not_import_a_layer_it_does_not_list() {
    let domain = "package domain\n\nimport (\n\t\"fmt\"\n\t\"example.com/app/internal/infra\"\n\t\"example.com/app/internal/domain/model\"\n\t\"github.com/x/y\"\n)\n";
    let infra =
        "package infra\n\nimport (\n\t\"example.com/app/internal/domain\"\n\t\"example.com/app/internal/app\"\n)\n";
    let repo = repo(&[
        ("go.mod", "module example.com/app\n"),
        ("internal/domain/d.go", domain),
        ("internal/infra/i.go", infra),
    ]);
    assert_eq!(
        lint(&repo, "internal/domain/d.go", LAYERS),
        ["go-layer-violation@5: layer `domain` must not import layer `infra` (`example.com/app/internal/infra`)"]
    );
    assert!(lint(&repo, "internal/infra/i.go", LAYERS).is_empty());
}

#[test]
fn a_layer_message_replaces_the_default_and_an_unlayered_file_is_free() {
    let domain = "package domain\n\nimport \"example.com/app/internal/infra\"\n";
    let other = "package other\n\nimport \"example.com/app/internal/infra\"\n";
    let repo = repo(&[
        ("go.mod", "module example.com/app\n"),
        ("internal/domain/d.go", domain),
        ("internal/other/o.go", other),
    ]);
    let options = r#"go_layers = [
        { name = "domain", paths = ["internal/domain"], message = "the domain stays pure" },
        { name = "infra", paths = ["internal/infra"] },
    ]"#;
    assert_eq!(
        lint(&repo, "internal/domain/d.go", options),
        ["go-layer-violation@3: the domain stays pure"]
    );
    assert!(lint(&repo, "internal/other/o.go", options).is_empty());
}

#[test]
fn imports_resolve_against_the_nearest_go_mod() {
    let nested =
        "package domain\n\nimport (\n\t\"example.com/sub/internal/infra\"\n\t\"example.com/app/internal/infra\"\n)\n";
    let repo = repo(&[
        ("go.mod", "module example.com/app\n"),
        ("sub/go.mod", "module example.com/sub\n"),
        ("sub/internal/domain/d.go", nested),
    ]);
    assert_eq!(
        lint(&repo, "sub/internal/domain/d.go", LAYERS),
        ["go-layer-violation@4: layer `domain` must not import layer `infra` (`example.com/sub/internal/infra`)"],
        "the nested module's own path resolves; the outer module's is another module"
    );
}

#[test]
fn layers_need_a_module_to_resolve_against() {
    let domain = "package domain\n\nimport \"example.com/app/internal/infra\"\n";
    let repo = repo(&[("internal/domain/d.go", domain)]);
    assert!(lint(&repo, "internal/domain/d.go", LAYERS).is_empty());
}

#[test]
fn the_rules_only_run_on_go_files() {
    let repo = repo(&[("go.mod", "module m\n"), ("x.rs", "fn main() { os::Exit(1); }\n")]);
    let path = repo.path().join("x.rs");
    let src = SourceFile {
        path,
        language: Language::Rust,
        content: "fn main() { os::Exit(1); }\n".into(),
    };
    let found = AstGrepEngine
        .lint(&src, &config(r#"go_forbidden_calls = [{ call = "os.Exit" }]"#))
        .unwrap();
    assert!(found.is_empty());
}

#[test]
fn a_bad_entry_fails_the_run_with_a_message_naming_it() {
    let repo = repo(&[("go.mod", "module m\n"), ("x.go", EXIT)]);
    let error = lint_result(&repo, "x.go", r#"go_forbidden_calls = [{ call = "Exit" }]"#).unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid ast-grep Go architecture config: go_forbidden_calls[0]: `call` must be `<import path>.<Name>` (such as `os.Exit`), got `Exit`"
    );
}

#[test]
fn the_config_is_part_of_the_cache_identity_and_a_go_file_is_keyed_by_its_module() {
    let repo = repo(&[("go.mod", "module example.com/app\n"), ("x.go", EXIT), ("x.rs", "")]);
    assert_eq!(AstGrepEngine.host_hook(), HostHook::GoModLint);
    assert_ne!(
        config(r#"go_forbidden_calls = [{ call = "os.Exit" }]"#).fingerprint(),
        config(r#"go_forbidden_calls = [{ call = "os.Getenv" }]"#).fingerprint()
    );
    let cfg = config("");
    let salt = AstGrepEngine
        .file_context(&repo.path().join("x.go"), &cfg)
        .expect("a Go file in a module is salted");
    assert!(salt.ends_with(":example.com/app"), "{salt}");
    assert_eq!(AstGrepEngine.file_context(&repo.path().join("x.rs"), &cfg), None);
}
