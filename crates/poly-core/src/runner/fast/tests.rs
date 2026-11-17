use poly_cache::{Namespace, ResultCache};

use super::*;
use crate::config::{Config, Kind};
use crate::engine::{Capabilities, Engine};
use crate::filter::SeverityRemap;

struct Stub {
    version: &'static str,
    run_context: &'static str,
}

impl Engine for Stub {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn languages(&self) -> &'static [Language] {
        &[]
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            lint: true,
            format: false,
            fix: false,
            project: false,
        }
    }

    fn version(&self) -> &str {
        self.version
    }

    fn run_context(&self, _cfg: &crate::config::EngineConfig) -> String {
        self.run_context.to_owned()
    }
}

fn engine_plan(stub: Stub, args: &toml::Table) -> EnginePlan {
    let config = Config::default().engine_config(&Language::Python, "stub", Kind::Lint);
    let mut args = args.clone();
    let context = stub.run_context;
    EnginePlan {
        engine: Box::new(stub),
        config,
        serialized_args: {
            if !context.is_empty() {
                args.insert("__run_context".to_owned(), toml::Value::String(context.to_owned()));
            }
            ResultCache::serialize_args(&args)
        },
        severity_remap: SeverityRemap::new(Vec::new()),
        provides_language_lint: true,
        format_ignore: None,
    }
}

fn key(stub: Stub, args: &toml::Table) -> PlanKey {
    base_key(Namespace::Lint, &Language::Python, &engine_plan(stub, args))
}

const BASE: Stub = Stub {
    version: "1",
    run_context: "",
};

#[test]
fn an_engine_version_bump_changes_the_key() {
    let args = toml::Table::new();
    assert_eq!(key(BASE, &args), key(BASE, &args));
    assert_ne!(key(BASE, &args), key(Stub { version: "2", ..BASE }, &args));
}

#[test]
fn an_engine_option_change_changes_the_key() {
    let mut other = toml::Table::new();
    other.insert("select".to_owned(), toml::Value::String("F401".to_owned()));
    assert_ne!(key(BASE, &toml::Table::new()), key(BASE, &other));
}

#[test]
fn a_change_in_the_engines_run_context_changes_the_key() {
    let args = toml::Table::new();
    assert_ne!(
        key(BASE, &args),
        key(
            Stub {
                run_context: "module tree v2",
                ..BASE
            },
            &args
        )
    );
}

#[test]
fn the_namespace_is_part_of_the_key() {
    let plan = engine_plan(BASE, &toml::Table::new());
    assert_ne!(
        base_key(Namespace::Lint, &Language::Python, &plan),
        base_key(Namespace::Fmt, &Language::Python, &plan)
    );
}

/// Segments are reused across plans by key, so the language they judged is part of it.
#[test]
fn the_language_is_part_of_the_key() {
    let plan = engine_plan(BASE, &toml::Table::new());
    assert_ne!(
        base_key(Namespace::Lint, &Language::C, &plan),
        base_key(Namespace::Lint, &Language::Cpp, &plan)
    );
}

#[test]
fn an_empty_payload_means_nothing_found_and_findings_round_trip() {
    assert!(decode_payload(&[]).is_some_and(|found| found.is_empty()));
    assert_eq!(encode_payload(&[]), Vec::<u8>::new());
    let finding = Diagnostic {
        engine: "ruff".to_owned(),
        code: Some("F401".to_owned()),
        severity: crate::engine::Severity::Warning,
        title: "unused".to_owned(),
        description: None,
        span: None,
        url: None,
        fix: Vec::new(),
        metadata: Default::default(),
    };
    let round_tripped = decode_payload(&encode_payload(std::slice::from_ref(&finding))).unwrap();
    assert_eq!(round_tripped.len(), 1);
    assert_eq!(round_tripped[0].code, finding.code);
    assert!(decode_payload(b"not json").is_none());
}

mod plans {
    //! What a record written by one plan may answer for another.

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::super::super::batch::BatchPrefill;
    use super::super::super::ctx::FormatCtx;
    use super::super::super::format_file::format_one;
    use super::super::super::plan::RunPlan;
    use super::super::super::types::FormatResult;
    use super::super::*;
    use crate::config::{EngineConfig, GlobalDefaults};
    use crate::engine::{Capabilities, Engine, FormatOutput};
    use crate::filter::SeverityRemap;

    /// A formatter that changes nothing, counts its runs, and declines every file when told to.
    struct Formatter {
        name: &'static str,
        declines: bool,
        runs: Arc<AtomicUsize>,
    }

    impl Engine for Formatter {
        fn name(&self) -> &'static str {
            self.name
        }

        fn languages(&self) -> &'static [Language] {
            &[Language::Python]
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                lint: false,
                format: true,
                fix: false,
                project: false,
            }
        }

        fn version(&self) -> &str {
            "1"
        }

        fn skip_reason(&self, _src: &SourceFile) -> Option<std::borrow::Cow<'static, str>> {
            self.declines.then_some("declined by the test".into())
        }

        fn format(&self, _src: &SourceFile, _cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(FormatOutput::Unchanged)
        }
    }

    pub(super) fn plan_of(engine: Box<dyn Engine>) -> EnginePlan {
        EnginePlan {
            engine,
            config: EngineConfig {
                globals: GlobalDefaults::default(),
                indent_width: 4,
                options: toml::Table::new(),
            },
            serialized_args: ResultCache::serialize_args(&toml::Table::new()),
            severity_remap: SeverityRemap::new(Vec::new()),
            provides_language_lint: true,
            format_ignore: None,
        }
    }

    pub(super) struct Tree {
        _directory: tempfile::TempDir,
        pub(super) cache_root: std::path::PathBuf,
        pub(super) file: DiscoveredFile,
    }

    impl Tree {
        pub(super) fn new(content: &str) -> Self {
            let directory = tempfile::tempdir().expect("tempdir");
            let path = directory.path().join("main.py");
            std::fs::write(&path, content).expect("write the file");
            // Older than the racy window, so the first record is trusted by stat.
            let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .and_then(|file| file.set_modified(old))
                .expect("age the file");
            Self {
                cache_root: directory.path().join("cache"),
                file: DiscoveredFile {
                    path,
                    language: Language::Python,
                    config_id: 0,
                },
                _directory: directory,
            }
        }
    }

    struct Chain {
        tree: Tree,
        first: Arc<AtomicUsize>,
        second: Arc<AtomicUsize>,
    }

    impl Chain {
        fn new() -> Self {
            Self {
                tree: Tree::new("x = 1\n"),
                first: Arc::new(AtomicUsize::new(0)),
                second: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// One `poly fmt` of the file by `first`, then `second` when `with_second`, over a cache
        /// reopened from disk.
        fn fmt(&self, first_declines: bool, with_second: bool) -> FormatResult {
            let cache = ResultCache::open(self.tree.cache_root.clone(), true).expect("open the cache");
            let mut engines = vec![plan_of(Box::new(Formatter {
                name: "first",
                declines: first_declines,
                runs: Arc::clone(&self.first),
            }))];
            if with_second {
                engines.push(plan_of(Box::new(Formatter {
                    name: "second",
                    declines: false,
                    runs: Arc::clone(&self.second),
                })));
            }
            let plan = RunPlan::serving(0, Language::Python, engines);
            let limits = SizeLimits::default();
            let replay = Replay {
                diagnostics: true,
                verdicts: true,
            };
            let fast = FastPath::new(&cache, Namespace::Fmt, &plan, &|_| 1u8, &limits, replay);
            let resolution = fast
                .as_ref()
                .map_or(Resolution::Unavailable, |fast| fast.resolve(&self.tree.file));
            let prefilled = BatchPrefill::default();
            let ctx = FormatCtx {
                plan: &plan,
                cache: &cache,
                write: false,
                fix_generated: false,
                collect_debug: false,
                act_on_generated: &[true],
                limits: &limits,
                prefilled: &prefilled,
                fast: fast.as_ref(),
            };
            let result = format_one(&ctx, &self.tree.file, resolution).expect("format");
            flush_stat_indexes(&cache);
            result
        }
    }

    /// `first` stands for an engine a `FormatScope` keeps away from the file, whose key is the
    /// unwrapped engine's: the chain's record must not say that `first` alone leaves the file as it is.
    #[test]
    fn a_fmt_record_does_not_answer_for_a_shorter_chain() {
        let chain = Chain::new();
        chain.fmt(false, true);
        assert_eq!(chain.first.load(Ordering::SeqCst), 1);
        chain.fmt(false, true);
        assert_eq!(
            chain.first.load(Ordering::SeqCst),
            1,
            "the same chain must be answered from its record"
        );

        chain.fmt(false, false);
        assert_eq!(
            chain.first.load(Ordering::SeqCst),
            2,
            "the remaining engine must format the file itself"
        );
    }

    #[test]
    fn a_fmt_record_does_not_count_a_file_its_only_remaining_engine_declines() {
        let chain = Chain::new();
        assert_eq!(chain.fmt(true, true).skipped, None);
        assert_eq!(chain.second.load(Ordering::SeqCst), 1);

        let narrowed = chain.fmt(true, false);
        assert_eq!(narrowed.skipped.as_deref(), Some("declined by the test"));
    }
}
