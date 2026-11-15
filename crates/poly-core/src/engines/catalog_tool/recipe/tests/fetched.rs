//! Scratch copies of a project: its config and linked directories reach the files it owns, never
//! a file outside it, and a project without fetched dependencies is skipped, not failed.

use super::*;

/// A formatter that needs `deps/` when asked, refuses a project config for a file outside the
/// project, and upper-cases everything else in place.
const PROJECT_FORMATTER: &str = r#"#!/bin/sh
for arg in "$@"; do
  [ -f "$arg" ] || continue
  case "$arg" in
    _outside/*) [ -f .formatter.exs ] && { echo "outside file saw the project formatter config" >&2; exit 2; } ;;
    *) [ -f deps/marker.txt ] || { echo "deps not reachable" >&2; exit 2; } ;;
  esac
  tr a-z A-Z < "$arg" > "$arg.tmp"
  mv "$arg.tmp" "$arg"
done
exit 0
"#;

fn project_tool(fx: &Fixture, recipe: serde_json::Value, script: &str) -> RecipeTool {
    let formatter = fx.script("formatter", script);
    let mut tool = fx.tool("$FILES", recipe, Role::Format, &fx.config(formatter));
    tool.cwd = std::fs::canonicalize(fx.dir.path().join("proj")).expect("project");
    tool
}

fn project(fx: &Fixture, formatter_config: &str) {
    std::fs::create_dir_all(fx.dir.path().join("proj")).expect("project");
    std::fs::write(fx.dir.path().join("proj/.formatter.exs"), formatter_config).expect("config");
}

#[test]
fn a_file_outside_the_project_is_formatted_without_the_projects_config() {
    let fx = Fixture::new();
    project(&fx, "[import_deps: [:dep]]\n");
    std::fs::create_dir_all(fx.dir.path().join("proj/deps")).expect("deps");
    std::fs::write(fx.dir.path().join("proj/deps/marker.txt"), "").expect("marker");
    let tool = project_tool(
        &fx,
        serde_json::json!({"config_files": [".formatter.exs"], "link_dirs": ["deps"]}),
        PROJECT_FORMATTER,
    );
    let inside = fx.file("proj/lib/a.txt", "hello\n");
    let outside = fx.file("loose/b.txt", "world\n");

    let results = tool.format_files(&refs(&[inside, outside]));

    assert!(matches!(&results[0], Ok(FormatOutput::Formatted(text)) if text == "HELLO\n"));
    assert!(matches!(&results[1], Ok(FormatOutput::Formatted(text)) if text == "WORLD\n"));
}

#[test]
fn linked_directories_reach_the_scratch_copy_of_the_projects_files() {
    let fx = Fixture::new();
    project(&fx, "[]\n");
    std::fs::create_dir_all(fx.dir.path().join("proj/deps")).expect("deps");
    std::fs::write(fx.dir.path().join("proj/deps/marker.txt"), "").expect("marker");
    let with = project_tool(&fx, serde_json::json!({"link_dirs": ["deps"]}), PROJECT_FORMATTER);
    let without = project_tool(&fx, serde_json::json!({}), PROJECT_FORMATTER);
    let file = fx.file("proj/lib/a.txt", "hello\n");

    assert!(matches!(
        &with.format_files(&[&file])[0],
        Ok(FormatOutput::Formatted(text)) if text == "HELLO\n"
    ));
    assert!(
        without.format_files(&[&file])[0].is_err(),
        "the unlinked copy has no deps"
    );
}

fn fetched_recipe() -> serde_json::Value {
    serde_json::json!({
        "config_files": [".formatter.exs"],
        "fetched": {"dir": "deps", "command": "mix deps.get", "config_mentions": "import_deps"}
    })
}

#[test]
fn unfetched_dependencies_skip_each_file_of_the_project_with_one_reason() {
    let fx = Fixture::new();
    project(&fx, "# import_deps is not used\n[import_deps: [:dep]]\n");
    let tool = project_tool(&fx, fetched_recipe(), PROJECT_FORMATTER);
    let a = fx.file("proj/lib/a.txt", "a\n");
    let b = fx.file("proj/test/b.txt", "b\n");

    let first = tool.unfetched_reason(&a.path).expect("skipped");
    assert!(
        first.starts_with("faketool: dependencies not fetched (run `mix deps.get` in "),
        "{first}"
    );
    assert!(first.ends_with("proj)"), "{first}");
    assert_eq!(tool.unfetched_reason(&b.path), Some(first));
}

#[test]
fn only_a_project_that_imports_dependencies_needs_them_fetched() {
    let fx = Fixture::new();
    project(&fx, "# import_deps: [:dep]\n[line_length: 100]\n");
    let tool = project_tool(&fx, fetched_recipe(), PROJECT_FORMATTER);
    let file = fx.file("proj/lib/a.txt", "a\n");
    assert_eq!(
        tool.unfetched_reason(&file.path),
        None,
        "a commented mention does not count"
    );
}

#[test]
fn fetched_dependencies_and_files_outside_the_project_are_not_skipped() {
    let fx = Fixture::new();
    project(&fx, "[import_deps: [:dep]]\n");
    let tool = project_tool(&fx, fetched_recipe(), PROJECT_FORMATTER);
    let outside = fx.file("loose/b.txt", "b\n");
    assert_eq!(tool.unfetched_reason(&outside.path), None);

    let fetched = Fixture::new();
    project(&fetched, "[import_deps: [:dep]]\n");
    std::fs::create_dir_all(fetched.dir.path().join("proj/deps")).expect("deps");
    let tool = project_tool(&fetched, fetched_recipe(), PROJECT_FORMATTER);
    let file = fetched.file("proj/lib/a.txt", "a\n");
    assert_eq!(tool.unfetched_reason(&file.path), None);
}
