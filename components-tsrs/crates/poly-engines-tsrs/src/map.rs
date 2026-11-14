//! The compiler's diagnostics as poly findings, grouped by the file the host knows them by.
//!
//! A finding is reported when it is an error or warning in a file the host handed over, that file is a source (not
//! a declaration file, not under `node_modules`, and not JavaScript unless the program holds JavaScript), and it is
//! not a package the project declares but the hermetic run lacks ([`Declared`]).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use poly_engine_api::engine::{Diagnostic, ProjectFile, Severity, Span};

use crate::ENGINE_NAME;
use crate::external::{Declared, quoted};
use crate::program::{Outcome, Raw};
use crate::vfs::tsrs_path;

/// `outcome`'s diagnostics under the paths of `files`, in file order and then position order.
pub(crate) fn findings(
    outcome: Outcome,
    files: &[ProjectFile],
    declared: &Declared,
    report_unresolved_modules: bool,
) -> Vec<(PathBuf, Vec<Diagnostic>)> {
    let by_name: HashMap<String, &ProjectFile> = files.iter().map(|file| (tsrs_path(&file.path), file)).collect();
    let mut grouped: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for raw in outcome.diagnostics {
        let name = raw.file.as_deref().unwrap_or(&outcome.config);
        if !reportable(name, outcome.allow_js) {
            continue;
        }
        if !report_unresolved_modules {
            let argument = raw.argument.as_deref().or_else(|| quoted(&raw.message));
            if declared.is_missing_install(raw.code, argument) {
                continue;
            }
        }
        let Some(file) = by_name.get(name) else {
            tracing::debug!(
                file = name,
                code = raw.code,
                "tsrs: diagnostic outside the project files"
            );
            continue;
        };
        grouped.entry(file.path.clone()).or_default().push(diagnostic(raw));
    }
    grouped.into_iter().collect()
}

/// Whether findings in `name` (as tsrs names it) are reported.
fn reportable(name: &str, allow_js: bool) -> bool {
    if name.split('/').any(|part| part == "node_modules") {
        return false;
    }
    let base = name.rsplit('/').next().unwrap_or(name);
    if is_declaration_file(base) {
        return false;
    }
    allow_js || !is_javascript(base)
}

/// `x.d.ts`, `x.d.mts`, `x.d.cts` and `x.d.<ext>.ts`.
fn is_declaration_file(base: &str) -> bool {
    base.contains(".d.") && [".ts", ".mts", ".cts"].iter().any(|ext| base.ends_with(ext))
}

fn is_javascript(base: &str) -> bool {
    [".js", ".jsx", ".mjs", ".cjs"].iter().any(|ext| base.ends_with(ext))
}

fn diagnostic(raw: Raw) -> Diagnostic {
    Diagnostic {
        engine: ENGINE_NAME.to_owned(),
        code: Some(format!("TS{}", raw.code)),
        severity: if raw.error { Severity::Error } else { Severity::Warning },
        title: raw.message,
        description: (!raw.chain.is_empty()).then(|| raw.chain.join("\n")),
        span: raw.span.map(|(start_line, start_col, end_line, end_col)| Span {
            start_line,
            start_col,
            end_line,
            end_col,
        }),
        url: None,
        fix: Vec::new(),
        metadata: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use poly_engine_api::language::Language;

    use super::*;

    fn raw(file: Option<&str>, code: i32, argument: Option<&str>) -> Raw {
        Raw {
            file: file.map(str::to_owned),
            span: Some((2, 3, 2, 8)),
            code,
            error: true,
            message: format!("message {code}"),
            chain: vec!["why".to_owned(), "  deeper".to_owned()],
            argument: argument.map(str::to_owned),
        }
    }

    fn file(path: &str) -> ProjectFile {
        ProjectFile {
            path: PathBuf::from(path),
            language: Language::TypeScript,
            content: "".into(),
        }
    }

    fn outcome(diagnostics: Vec<Raw>, allow_js: bool) -> Outcome {
        Outcome {
            config: tsrs_path(std::path::Path::new("/w/tsconfig.json")),
            allow_js,
            diagnostics,
        }
    }

    #[test]
    fn declaration_files_node_modules_and_unrequested_javascript_are_not_reported() {
        for (name, allow_js, reported) in [
            ("/w/src/a.ts", false, true),
            ("/w/src/a.tsx", false, true),
            ("/w/tsconfig.json", false, true),
            ("/w/src/a.d.ts", false, false),
            ("/w/src/a.d.mts", true, false),
            ("/w/src/styles.d.css.ts", true, false),
            ("/w/node_modules/p/index.ts", true, false),
            ("/w/src/a.js", false, false),
            ("/w/src/a.js", true, true),
            ("/w/src/data.ts", false, true),
        ] {
            assert_eq!(reportable(name, allow_js), reported, "{name} allow_js={allow_js}");
        }
    }

    #[test]
    fn findings_carry_code_severity_span_and_the_flattened_chain() {
        let files = [file("/w/src/a.ts")];
        let name = tsrs_path(std::path::Path::new("/w/src/a.ts"));
        let found = findings(
            outcome(vec![raw(Some(&name), 2322, None)], false),
            &files,
            &Declared::default(),
            false,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, PathBuf::from("/w/src/a.ts"));
        let d = &found[0].1[0];
        assert_eq!((d.engine.as_str(), d.code.as_deref()), ("tsrs", Some("TS2322")));
        assert_eq!(d.severity, Severity::Error);
        assert_eq!(d.title, "message 2322");
        assert_eq!(d.description.as_deref(), Some("why\n  deeper"));
        let span = d.span.expect("a span");
        assert_eq!(
            (span.start_line, span.start_col, span.end_line, span.end_col),
            (2, 3, 2, 8)
        );
    }

    #[test]
    fn a_diagnostic_without_a_file_lands_on_the_tsconfig_and_one_outside_the_files_is_dropped() {
        let files = [file("/w/tsconfig.json"), file("/w/src/a.ts")];
        let outside = tsrs_path(std::path::Path::new("/elsewhere/b.ts"));
        let found = findings(
            outcome(vec![raw(None, 5009, None), raw(Some(&outside), 2322, None)], false),
            &files,
            &Declared::default(),
            false,
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, PathBuf::from("/w/tsconfig.json"));
    }

    #[test]
    fn a_missing_install_is_dropped_unless_asked_for() {
        let files = [file("/w/src/a.ts")];
        let name = tsrs_path(std::path::Path::new("/w/src/a.ts"));
        let manifest = ProjectFile {
            path: PathBuf::from("/w/package.json"),
            language: Language::Other("package.json".into()),
            content: r#"{ "dependencies": { "left-pad": "1.0.0" } }"#.into(),
        };
        let declared = Declared::from_files(&[manifest]);
        let missing = || {
            outcome(
                vec![
                    raw(Some(&name), 2307, Some("left-pad")),
                    raw(Some(&name), 2307, Some("typo")),
                ],
                false,
            )
        };
        let hidden = findings(missing(), &files, &declared, false);
        assert_eq!(hidden[0].1.len(), 1, "only the undeclared package is reported");
        let shown = findings(missing(), &files, &declared, true);
        assert_eq!(shown[0].1.len(), 2);
    }

    #[test]
    fn the_module_is_read_from_the_message_when_no_argument_was_kept() {
        let files = [file("/w/src/a.ts")];
        let name = tsrs_path(std::path::Path::new("/w/src/a.ts"));
        let manifest = ProjectFile {
            path: PathBuf::from("/w/package.json"),
            language: Language::Other("package.json".into()),
            content: r#"{ "dependencies": { "left-pad": "1.0.0" } }"#.into(),
        };
        let mut diagnostic = raw(Some(&name), 2307, None);
        diagnostic.message = "Cannot find module 'left-pad' or its corresponding type declarations.".to_owned();
        let found = findings(
            outcome(vec![diagnostic], false),
            &files,
            &Declared::from_files(&[manifest]),
            false,
        );
        assert!(found.is_empty());
    }
}
