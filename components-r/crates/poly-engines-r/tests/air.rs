//! Insta fixtures for the air formatter.
//!
//! The expected output of every fixture is what the `air format` CLI writes for the same input and settings, built
//! from the same revision (`air_r_formatter` 9b7627a): the unformatted file is formatted at air's own defaults (a line
//! width of 80, which `line_width = 80` selects here, since poly's default is 120), and the `air.toml` fixtures
//! are the CLI's output for a project holding that file.

use std::path::Path;

use poly_engine_api::config::{EngineConfig, GlobalDefaults};
use poly_engine_api::engine::{Engine, FormatOutput, SourceFile};
use poly_engine_api::language::Language;
use poly_engines_r::AirEngine;

/// Unformatted R: spacing, `=` assignment, a one-line `if`/`else`, a call too wide for 80 columns and a pipe.
///
/// No trailing whitespace on any line, so the `trailing-whitespace` hook does not rewrite this literal.
const KNOWN_UNFORMATTED: &str = "f <- function(x,y){\n    z = x+y\n  if(z>10){ print( \"big\" ) } else print(\"small\")\n  z\n}\nresult <- some_function(argument_one = 1, argument_two = \"two\", argument_three = c(1, 2, 3), argument_four = TRUE)\ndf %>% filter(a>1) %>% select(a,b)\n";

fn config(options: &str) -> EngineConfig {
    EngineConfig {
        globals: GlobalDefaults::default(),
        indent_width: Language::R.default_indent_width(),
        options: options.parse().expect("test options are TOML"),
    }
}

fn source(path: &Path, content: &str) -> SourceFile {
    SourceFile {
        path: path.to_path_buf(),
        language: Language::R,
        content: content.into(),
    }
}

fn formatted(output: FormatOutput) -> String {
    match output {
        FormatOutput::Formatted(text) => text,
        FormatOutput::Unchanged => panic!("expected the fixture to be reformatted"),
    }
}

#[test]
fn known_unformatted_file_formats_as_the_air_cli_does() {
    let dir = tempfile::tempdir().unwrap();
    let src = source(&dir.path().join("fixture.R"), KNOWN_UNFORMATTED);
    let text = formatted(AirEngine.format(&src, &config("line_width = 80")).unwrap());
    insta::assert_snapshot!("air_known_unformatted_output", text);
}

#[test]
fn a_formatted_file_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let text = "x <- c(1, 2, 3)\n\nf <- function(a) {\n  a + 1\n}\n";
    let src = source(&dir.path().join("clean.R"), text);
    assert!(matches!(
        AirEngine.format(&src, &config("")).unwrap(),
        FormatOutput::Unchanged
    ));
}

#[test]
fn poly_defaults_to_120_columns_where_air_defaults_to_80() {
    let dir = tempfile::tempdir().unwrap();
    let call = "result <- some_function(argument_one = 1, argument_two = \"two\", argument_three = c(1, 2, 3), argument_four = TRUE)\n";
    let src = source(&dir.path().join("wide.R"), call);
    assert!(
        matches!(AirEngine.format(&src, &config("")).unwrap(), FormatOutput::Unchanged),
        "a 113-column call fits poly's default width"
    );
    let wrapped = formatted(AirEngine.format(&src, &config("line_width = 80")).unwrap());
    assert!(
        wrapped.starts_with("result <- some_function(\n  argument_one = 1,\n"),
        "{wrapped}"
    );
}

#[test]
fn a_project_air_toml_is_the_whole_configuration() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("air.toml"),
        "[format]\nindent-style = \"tab\"\nline-width = 40\n",
    )
    .unwrap();
    let nested = dir.path().join("R");
    std::fs::create_dir(&nested).unwrap();
    let src = source(&nested.join("fixture.R"), KNOWN_UNFORMATTED);
    let text = formatted(AirEngine.format(&src, &config("")).unwrap());
    assert_eq!(
        text,
        "f <- function(x, y) {\n\tz <- x + y\n\tif (z > 10) {\n\t\tprint(\"big\")\n\t} else {\n\t\tprint(\"small\")\n\t}\n\tz\n}\nresult <- some_function(\n\targument_one = 1,\n\targument_two = \"two\",\n\targument_three = c(1, 2, 3),\n\targument_four = TRUE\n)\ndf %>% filter(a > 1) %>% select(a, b)\n"
    );
}

#[test]
fn an_air_toml_without_a_width_keeps_air_s_own_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("air.toml"), "[format]\nindent-width = 4\n").unwrap();
    let src = source(
        &dir.path().join("wide.R"),
        "result <- some_function(argument_one = 1, argument_two = \"two\", argument_three = c(1, 2, 3))\n",
    );
    let text = formatted(AirEngine.format(&src, &config("")).unwrap());
    assert_eq!(
        text,
        "result <- some_function(\n    argument_one = 1,\n    argument_two = \"two\",\n    argument_three = c(1, 2, 3)\n)\n"
    );
}

#[test]
fn engine_options_override_the_project_config_and_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let text = "x <- 1\nf <- function() {\n  y <- 2\n}\n";
    let src = source(&dir.path().join("a.R"), text);
    let equal = formatted(AirEngine.format(&src, &config("assignment_style = \"equal\"")).unwrap());
    assert_eq!(equal, "x = 1\nf = function() {\n  y = 2\n}\n");

    std::fs::write(dir.path().join("air.toml"), "[format]\nassignment-style = \"arrow\"\n").unwrap();
    let src = source(&dir.path().join("a.R"), "x = 1\n");
    assert!(
        matches!(
            AirEngine
                .format(&src, &config("assignment_style = \"preserve\""))
                .unwrap(),
            FormatOutput::Unchanged
        ),
        "the option wins over the air.toml: `preserve` keeps the `=` the file wrote"
    );
    let arrow = formatted(AirEngine.format(&src, &config("")).unwrap());
    assert_eq!(arrow, "x <- 1\n", "without the option the air.toml's `arrow` applies");
}

#[test]
fn a_file_that_does_not_parse_is_an_error_not_a_clean_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = source(&dir.path().join("bad.R"), "f <- function( {\n");
    let chain = format!("{:#}", AirEngine.format(&src, &config("")).unwrap_err());
    assert!(chain.contains("bad.R"), "{chain}");
    assert!(chain.contains("air could not parse the file"), "{chain}");
    assert!(
        chain.contains("Expected a parameter name but instead found '{'"),
        "{chain}"
    );
}

#[test]
fn air_s_default_exclude_patterns_skip_generated_and_vendored_files() {
    let dir = tempfile::tempdir().unwrap();
    let skip = |relative: &str| AirEngine.skip_reason(&source(&dir.path().join(relative), "x   <-   1\n"));
    assert_eq!(
        skip("R/RcppExports.R").as_deref(),
        Some("excluded by air's `**/RcppExports.R` pattern")
    );
    assert_eq!(
        skip("R/import-standalone-types.R").as_deref(),
        Some("excluded by air's `**/import-standalone-*.R` pattern")
    );
    assert_eq!(
        skip("renv/activate.R").as_deref(),
        Some("excluded by air's `**/renv/` pattern")
    );
    assert_eq!(skip("R/analysis.R"), None);
}

#[test]
fn the_project_air_toml_adds_exclude_patterns_and_can_drop_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("air.toml"),
        "[format]\nexclude = [\"generated/\"]\ndefault-exclude = false\n",
    )
    .unwrap();
    let skip = |relative: &str| AirEngine.skip_reason(&source(&dir.path().join(relative), "x   <-   1\n"));
    assert_eq!(
        skip("generated/a.R").as_deref(),
        Some("excluded by air's `generated/` pattern")
    );
    assert_eq!(
        skip("R/RcppExports.R"),
        None,
        "default-exclude = false lifts the defaults"
    );
}

#[test]
fn the_format_cache_key_follows_the_air_toml() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.R");
    let cfg = config("");
    assert_eq!(
        AirEngine.format_input_salt(&file, &cfg),
        None,
        "no air.toml, nothing outside the file"
    );
    std::fs::write(dir.path().join("air.toml"), "[format]\nline-width = 100\n").unwrap();
    let first = AirEngine
        .format_input_salt(&file, &cfg)
        .expect("the air.toml is part of the key");
    std::fs::write(dir.path().join("air.toml"), "[format]\nline-width = 90\n").unwrap();
    let second = AirEngine
        .format_input_salt(&file, &cfg)
        .expect("the air.toml is part of the key");
    assert_ne!(first, second);
}

#[test]
fn an_invalid_air_toml_is_an_error_naming_the_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("air.toml"), "[format]\nline-width = 0\n").unwrap();
    let src = source(&dir.path().join("a.R"), "x <- 1\n");
    let chain = format!("{:#}", AirEngine.format(&src, &config("")).unwrap_err());
    assert!(chain.contains("invalid air configuration"), "{chain}");
}
