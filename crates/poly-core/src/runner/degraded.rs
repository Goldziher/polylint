//! The files a wanted native tool should have handled and did not, because it was absent (ADR 0075).
//!
//! A missing `rustfmt`, `gofmt` or `shfmt` hands its language to the tier-2 reindenter, and a missing
//! `shellcheck` lints nothing (ADR 0014). Both are by design, and both read as a pass: the only trace
//! was a `tracing` warning that hook output and CI logs bury. The run report names them instead.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rustc_hash::FxHashSet;
use serde::{Serialize, Serializer};

use crate::discover::DiscoveredFile;
use crate::language::Language;
use crate::runner::plan::RunPlan;

/// Why a [`DegradedTool`] did not handle its files. The only cause today: the probe that decides
/// whether a native tool runs found no binary.
pub const MISSING_TOOL_REASON: &str = "not found on PATH";

/// One native tool that was configured or on by default and absent, and the files it should have
/// handled.
///
/// **Not a skip, and not charged to `--deny-skips`.** Every one of these files was still processed:
/// a format run gave it to the tier-2 reindenter and a lint run ran the other engines over it. A
/// skip says poly did not look at a file; this says poly looked with less than the configured tool.
/// Counting it as skipped would fail every `--deny-skips` gate on a host without the toolchain,
/// which is exactly the host ADR 0014 promises keeps working. A file the run did skip or failed on
/// is reported there and left out of here, so a file is never named twice for one cause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct DegradedTool {
    /// The binary poly looked for (`rustfmt`, `shellcheck`).
    pub tool: String,
    /// The language whose files it should have handled, serialized by its config id (`rust`).
    #[serde(serialize_with = "language_id")]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub language: Language,
    /// Why the tool did not run: [`MISSING_TOOL_REASON`].
    pub reason: String,
    /// The files handled without it, sorted.
    pub files: Vec<PathBuf>,
}

fn language_id<S: Serializer>(language: &Language, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(language.id())
}

/// The degraded tools of a run, one entry per `(tool, language)`, ordered by tool then language.
///
/// Derived from the plan rather than from the engine call, so a file served from the cache is
/// reported exactly like one the run recomputed. `unhandled` holds the files the run reports as
/// skipped or failed, which are left out (see [`DegradedTool`]).
pub(super) fn degraded_tools(
    files: &[DiscoveredFile],
    plan: &RunPlan,
    unhandled: &FxHashSet<&Path>,
) -> Vec<DegradedTool> {
    let mut groups: BTreeMap<(&'static str, &str), (&Language, Vec<PathBuf>)> = BTreeMap::new();
    for file in files {
        let tools = plan.missing_tools(file.config_id, &file.language);
        if tools.is_empty() || unhandled.contains(file.path.as_path()) {
            continue;
        }
        for &tool in tools {
            groups
                .entry((tool, file.language.id()))
                .or_insert_with(|| (&file.language, Vec::new()))
                .1
                .push(file.path.clone());
        }
    }
    groups
        .into_iter()
        .map(|((tool, _), (language, mut files))| {
            files.sort();
            DegradedTool {
                tool: tool.to_owned(),
                language: language.clone(),
                reason: MISSING_TOOL_REASON.to_owned(),
                files,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, language: Language) -> DiscoveredFile {
        DiscoveredFile {
            path: PathBuf::from(path),
            language,
            config_id: 0,
        }
    }

    #[test]
    fn names_every_file_a_missing_tool_should_have_handled() {
        let plan = RunPlan::serving(0, Language::Rust, Vec::new()).missing_tool(0, Language::Rust, "rustfmt");
        let files = [
            file("src/b.rs", Language::Rust),
            file("src/a.rs", Language::Rust),
            file("main.py", Language::Python),
        ];

        let degraded = degraded_tools(&files, &plan, &FxHashSet::default());

        assert_eq!(
            degraded,
            [DegradedTool {
                tool: "rustfmt".to_owned(),
                language: Language::Rust,
                reason: MISSING_TOOL_REASON.to_owned(),
                files: vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")],
            }]
        );
    }

    #[test]
    fn leaves_out_files_the_run_skipped_or_failed_on() {
        let plan = RunPlan::serving(0, Language::Go, Vec::new()).missing_tool(0, Language::Go, "gofmt");
        let files = [file("a.go", Language::Go), file("b.go", Language::Go)];
        let unhandled: FxHashSet<&Path> = [Path::new("a.go")].into_iter().collect();

        let degraded = degraded_tools(&files, &plan, &unhandled);

        assert_eq!(degraded.len(), 1);
        assert_eq!(degraded[0].files, [PathBuf::from("b.go")]);
    }

    #[test]
    fn is_empty_when_no_plan_misses_a_tool() {
        let plan = RunPlan::serving(0, Language::Rust, Vec::new());

        let degraded = degraded_tools(&[file("src/lib.rs", Language::Rust)], &plan, &FxHashSet::default());

        assert!(degraded.is_empty(), "{degraded:?}");
    }

    #[test]
    fn serializes_the_language_by_its_config_id() {
        let entry = DegradedTool {
            tool: "shellcheck".to_owned(),
            language: Language::Shell,
            reason: MISSING_TOOL_REASON.to_owned(),
            files: vec![PathBuf::from("run.sh")],
        };

        let json = serde_json::to_value(&entry).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "tool": "shellcheck",
                "language": "shell",
                "reason": "not found on PATH",
                "files": ["run.sh"],
            })
        );
    }
}
