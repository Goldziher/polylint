//! Insta fixtures for the Lua engines: a known-unformatted file for StyLua and a known-bad file for selene,
//! plus the config layering both read from a project.

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Diagnostic, Engine, FormatOutput, SourceFile};
use poly_engine_api::host_hooks::native_config;
use poly_engine_api::language::Language;
use poly_engines_lua::{SeleneEngine, StyluaEngine};

fn cfg(options: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: 4,
        options: options.parse().unwrap(),
    }
}

fn source(path: &str, content: &str) -> SourceFile {
    SourceFile {
        path: path.into(),
        language: Language::Lua,
        content: content.into(),
    }
}

/// Spacing, quotes and layout all off: StyLua should normalise every one of them.
const KNOWN_UNFORMATTED: &str = "\
local   function greet( name )
    local   message=\"hello, \"..name
  print(message)
    return   message
end

if   x   then   print( 'yes' )   end
";

/// One unused local and one read of a name nothing defines.
const KNOWN_BAD: &str = "\
local unused = 1

local function greet(name)
    return \"hello \" .. name .. unknown_value
end

greet(\"x\")
";

fn render(diagnostics: &[Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|d| {
            let span = d.span.expect("selene findings carry a span");
            format!(
                "{}:{}-{}:{} {:?} {} {}\n",
                span.start_line,
                span.start_col,
                span.end_line,
                span.end_col,
                d.severity,
                d.code.as_deref().unwrap_or("-"),
                d.title
            )
        })
        .collect()
}

#[test]
fn stylua_known_unformatted_snapshot() {
    let src = source("known_unformatted.lua", KNOWN_UNFORMATTED);
    let formatted = match StyluaEngine.format(&src, &cfg("indent_type = \"Spaces\"\n")).unwrap() {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => panic!("the fixture is unformatted, StyLua must change it"),
    };
    insta::assert_snapshot!("stylua_known_unformatted_output", formatted);

    let again = source("known_unformatted.lua", &formatted);
    assert!(
        matches!(
            StyluaEngine.format(&again, &cfg("indent_type = \"Spaces\"\n")).unwrap(),
            FormatOutput::Unchanged
        ),
        "formatting is not idempotent"
    );
}

#[test]
fn selene_known_bad_snapshot() {
    let diagnostics = SeleneEngine
        .lint(&source("known_bad.lua", KNOWN_BAD), &cfg(""))
        .unwrap();
    insta::assert_snapshot!("selene_known_bad_diagnostics", render(&diagnostics));
}

#[test]
fn selene_options_change_what_is_reported() {
    let src = source("known_bad.lua", KNOWN_BAD);
    let ignored = SeleneEngine
        .lint(&src, &cfg("ignore = [\"unused_variable\"]\n"))
        .unwrap();
    assert_eq!(
        render(&ignored),
        "4:32-4:45 Error undefined_variable `unknown_value` is not defined\n"
    );
    let denied = SeleneEngine
        .lint(&src, &cfg("[lints]\nunused_variable = \"deny\"\n"))
        .unwrap();
    assert!(denied.iter().any(
        |d| d.code.as_deref() == Some("unused_variable") && d.severity == poly_engine_api::engine::Severity::Error
    ));
}

#[test]
fn selene_reports_a_syntax_error_and_an_unsupported_std() {
    let broken = SeleneEngine
        .lint(&source("broken.lua", "local = = 1\n"), &cfg(""))
        .unwrap();
    assert!(broken.iter().any(|d| d.code.as_deref() == Some("syntax")), "{broken:?}");
    let roblox = SeleneEngine
        .lint(&source("a.lua", "local a = 1\n"), &cfg("std = \"roblox\"\n"))
        .unwrap();
    assert_eq!(roblox.len(), 1);
    assert_eq!(roblox[0].code.as_deref(), Some("unsupported-std"));
}

#[test]
fn project_config_files_are_honoured_and_poly_toml_wins() {
    let repo = tempfile::tempdir().unwrap();
    std::fs::create_dir(repo.path().join(".git")).unwrap();
    std::fs::write(
        repo.path().join("selene.toml"),
        "[lints]\nunused_variable = \"allow\"\n",
    )
    .unwrap();
    std::fs::write(repo.path().join("stylua.toml"), "quote_style = \"ForceSingle\"\n").unwrap();
    native_config::forget();
    let path = repo.path().join("a.lua");
    let path = path.to_str().unwrap();

    let lint = |options: &str| SeleneEngine.lint(&source(path, KNOWN_BAD), &cfg(options)).unwrap();
    assert!(lint("").iter().all(|d| d.code.as_deref() != Some("unused_variable")));
    assert!(
        lint("[lints]\nunused_variable = \"warn\"\n")
            .iter()
            .any(|d| d.code.as_deref() == Some("unused_variable"))
    );

    let content = "local a = \"x\"\n";
    let src = source(path, content);
    let merged =
        poly_engine_api::host_hooks::format_config(StyluaEngine.host_hook(), StyluaEngine.name(), &src, &cfg(""));
    match StyluaEngine.format(&src, &merged).unwrap() {
        FormatOutput::Formatted(text) => assert_eq!(text, "local a = 'x'\n"),
        FormatOutput::Unchanged => panic!("stylua.toml's quote_style did not reach the formatter"),
    }
}
