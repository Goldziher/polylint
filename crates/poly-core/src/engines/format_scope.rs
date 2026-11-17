//! Hand named files to a more specific formatter.
//!
//! `pyproject-fmt` formats `pyproject.toml` with opinions of its own (key order,
//! array layout), and the language's tier-one formatter (`taplo`) formats every
//! TOML file with different ones. Run in a chain the two never settle: each
//! rewrites what the other produced. [`FormatScope`] wraps the tier-one formatter
//! and makes it decline the files the specific formatter claims, so each file has
//! exactly one formatter. Everything but formatting those files is delegated
//! unchanged — the wrapped engine still lints them.

use globset::GlobSet;
use std::borrow::Cow;

use crate::config::EngineConfig;
use crate::engine::{
    BatchSupport, Batching, Capabilities, CatalogTakeover, Diagnostic, Engine, FormatOutput, OptionKeys, OptionTable,
    PlanPolicy, ProjectAnalysis, ProjectInputs, ProjectRequest, SourceFile, WireSelection,
};
use crate::language::Language;

/// An engine that does not format the files `claimed` matches.
pub struct FormatScope {
    inner: Box<dyn Engine>,
    claimed: GlobSet,
}

impl FormatScope {
    /// Wrap `inner` so it leaves the files matching `claimed` to someone else.
    pub fn new(inner: Box<dyn Engine>, claimed: GlobSet) -> Self {
        FormatScope { inner, claimed }
    }

    fn claims(&self, src: &SourceFile) -> bool {
        self.claimed.is_match(poly_engine_api::run_root::relative(&src.path))
    }
}

impl Engine for FormatScope {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    fn languages(&self) -> &'static [Language] {
        self.inner.languages()
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn option_keys(&self, table: OptionTable) -> OptionKeys {
        self.inner.option_keys(table)
    }

    fn version(&self) -> &str {
        self.inner.version()
    }

    fn skip_reason(&self, src: &SourceFile) -> Option<Cow<'static, str>> {
        if self.claims(src) {
            return None;
        }
        self.inner.skip_reason(src)
    }

    fn lint(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
        self.inner.lint(src, cfg)
    }

    fn format(&self, src: &SourceFile, cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
        if self.claims(src) {
            return Ok(FormatOutput::Unchanged);
        }
        self.inner.format(src, cfg)
    }

    fn lint_cache_salt(&self, src: &SourceFile) -> Option<String> {
        self.inner.lint_cache_salt(src)
    }

    fn format_config<'a>(&self, src: &SourceFile, cfg: &'a EngineConfig) -> std::borrow::Cow<'a, EngineConfig> {
        self.inner.format_config(src, cfg)
    }

    fn format_input_salt(&self, path: &std::path::Path, cfg: &EngineConfig) -> Option<String> {
        self.inner.format_input_salt(path, cfg)
    }

    fn file_context(&self, path: &std::path::Path, cfg: &EngineConfig) -> Option<String> {
        self.inner.file_context(path, cfg)
    }

    fn overridden_hooks(&self) -> u32 {
        self.inner.overridden_hooks()
    }

    fn run_context(&self, cfg: &EngineConfig) -> String {
        self.inner.run_context(cfg)
    }

    fn host_hook(&self) -> crate::host_hooks::HostHook {
        self.inner.host_hook()
    }

    fn lint_input_salt(&self, path: &std::path::Path, cfg: &EngineConfig) -> Option<String> {
        self.inner.lint_input_salt(path, cfg)
    }

    fn lint_cache_version(&self, src: &SourceFile) -> Cow<'_, str> {
        self.inner.lint_cache_version(src)
    }

    // Each facet is offered exactly when the wrapped engine has it, and answers by forwarding.
    fn batching(&self) -> Option<&dyn Batching> {
        self.inner.batching().map(|_| self as &dyn Batching)
    }

    fn project_analysis(&self) -> Option<&dyn ProjectAnalysis> {
        self.inner.project_analysis().map(|_| self as &dyn ProjectAnalysis)
    }

    fn plan_policy(&self) -> Option<&dyn PlanPolicy> {
        self.inner.plan_policy().map(|_| self as &dyn PlanPolicy)
    }

    fn catalog_takeover(&self) -> Option<&dyn CatalogTakeover> {
        self.inner.catalog_takeover().map(|_| self as &dyn CatalogTakeover)
    }
}

impl Batching for FormatScope {
    fn batch_support(&self, cfg: &EngineConfig) -> BatchSupport {
        self.inner.batch_support(cfg)
    }

    fn format_batch(
        &self,
        batch: &[SourceFile],
        cfg: &EngineConfig,
    ) -> anyhow::Result<Vec<anyhow::Result<FormatOutput>>> {
        let kept: Vec<SourceFile> = batch.iter().filter(|src| !self.claims(src)).cloned().collect();
        let mut formatted = self.inner.format_batch(&kept, cfg)?.into_iter();
        Ok(batch
            .iter()
            .map(|src| {
                if self.claims(src) {
                    Ok(FormatOutput::Unchanged)
                } else {
                    formatted
                        .next()
                        .unwrap_or_else(|| Err(anyhow::anyhow!("batch answered fewer files than it was given")))
                }
            })
            .collect())
    }

    fn lint_batch(
        &self,
        batch: &[SourceFile],
        cfg: &EngineConfig,
    ) -> anyhow::Result<Vec<anyhow::Result<Vec<Diagnostic>>>> {
        self.inner.lint_batch(batch, cfg)
    }

    fn batch_shard_floor(&self) -> usize {
        self.inner.batch_shard_floor()
    }
}

impl ProjectAnalysis for FormatScope {
    fn project_check(&self, req: &ProjectRequest<'_>) -> anyhow::Result<Vec<(std::path::PathBuf, Vec<Diagnostic>)>> {
        self.inner.project_check(req)
    }

    fn project_inputs(&self) -> ProjectInputs {
        self.inner.project_inputs()
    }

    fn selection(&self) -> Option<WireSelection> {
        self.inner.selection()
    }
}

impl PlanPolicy for FormatScope {
    fn provides_language_lint(&self, language: &Language, cfg: &EngineConfig) -> bool {
        self.inner.provides_language_lint(language, cfg)
    }

    fn provides_language_format(&self, language: &Language, cfg: &EngineConfig) -> bool {
        self.inner.provides_language_format(language, cfg)
    }

    fn self_manages_enabled(&self) -> bool {
        self.inner.self_manages_enabled()
    }

    fn missing_tool(&self, language: &Language, cfg: &EngineConfig) -> Option<&'static str> {
        self.inner.missing_tool(language, cfg)
    }

    fn supersedes_generic_formatter(&self) -> bool {
        self.inner.supersedes_generic_formatter()
    }
}

impl CatalogTakeover for FormatScope {
    fn takes_over_tier_one(&self) -> bool {
        self.inner.takes_over_tier_one()
    }

    fn claimed_paths(&self) -> Vec<String> {
        self.inner.claimed_paths()
    }

    fn supplements_tier_one(&self) -> bool {
        self.inner.supplements_tier_one()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use globset::{Glob, GlobSetBuilder};

    use super::*;
    use crate::config::GlobalDefaults;

    /// Upper-cases every file it formats.
    struct Upper;

    impl Engine for Upper {
        fn name(&self) -> &'static str {
            "upper"
        }

        fn languages(&self) -> &'static [Language] {
            &[Language::Toml]
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
            "test"
        }

        fn format(&self, src: &SourceFile, _cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
            Ok(FormatOutput::Formatted(src.content.to_uppercase()))
        }

        fn batching(&self) -> Option<&dyn Batching> {
            Some(self)
        }
    }

    impl Batching for Upper {
        fn format_batch(
            &self,
            batch: &[SourceFile],
            cfg: &EngineConfig,
        ) -> anyhow::Result<Vec<anyhow::Result<FormatOutput>>> {
            Ok(batch.iter().map(|src| self.format(src, cfg)).collect())
        }
    }

    fn scoped() -> FormatScope {
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new("**/pyproject.toml").expect("glob"));
        FormatScope::new(Box::new(Upper), builder.build().expect("set"))
    }

    fn source(path: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            language: Language::Toml,
            content: Arc::from("a = 1\n"),
        }
    }

    fn cfg() -> EngineConfig {
        EngineConfig {
            globals: GlobalDefaults::default(),
            indent_width: 2,
            options: toml::Table::new(),
        }
    }

    #[test]
    fn a_claimed_file_is_left_to_the_specific_formatter() {
        let engine = scoped();
        assert!(matches!(
            engine.format(&source("svc/pyproject.toml"), &cfg()),
            Ok(FormatOutput::Unchanged)
        ));
        assert!(matches!(
            engine.format(&source("svc/other.toml"), &cfg()),
            Ok(FormatOutput::Formatted(text)) if text == "A = 1\n"
        ));
    }

    #[test]
    fn a_batch_keeps_its_order_around_the_claimed_files() {
        let engine = scoped();
        let batch = [source("a.toml"), source("x/pyproject.toml"), source("b.toml")];
        let out = engine.format_batch(&batch, &cfg()).expect("batch");
        assert_eq!(out.len(), 3);
        assert!(matches!(&out[0], Ok(FormatOutput::Formatted(_))));
        assert!(matches!(&out[1], Ok(FormatOutput::Unchanged)));
        assert!(matches!(&out[2], Ok(FormatOutput::Formatted(_))));
    }

    /// Records the name of every trait method called on it, answering each with a non-default value
    /// where the type allows so a dropped forward is also visible in the result.
    struct Recorder(Arc<std::sync::Mutex<Vec<&'static str>>>);

    impl Recorder {
        fn note(&self, method: &'static str) {
            self.0.lock().expect("lock").push(method);
        }
    }

    impl Engine for Recorder {
        fn name(&self) -> &'static str {
            self.note("name");
            "recorder"
        }
        fn languages(&self) -> &'static [Language] {
            self.note("languages");
            &[Language::Toml]
        }
        fn capabilities(&self) -> Capabilities {
            self.note("capabilities");
            Capabilities {
                lint: true,
                format: true,
                fix: false,
                project: false,
            }
        }
        fn option_keys(&self, _table: OptionTable) -> OptionKeys {
            self.note("option_keys");
            OptionKeys::UNCHECKED
        }
        fn overridden_hooks(&self) -> u32 {
            self.note("overridden_hooks");
            0
        }
        fn version(&self) -> &str {
            self.note("version");
            "recorder"
        }
        fn run_context(&self, _cfg: &EngineConfig) -> String {
            self.note("run_context");
            String::new()
        }
        fn file_context(&self, _path: &std::path::Path, _cfg: &EngineConfig) -> Option<String> {
            self.note("file_context");
            None
        }
        fn host_hook(&self) -> crate::host_hooks::HostHook {
            self.note("host_hook");
            crate::host_hooks::HostHook::None
        }
        fn skip_reason(&self, _src: &SourceFile) -> Option<Cow<'static, str>> {
            self.note("skip_reason");
            None
        }
        fn lint_input_salt(&self, _path: &std::path::Path, _cfg: &EngineConfig) -> Option<String> {
            self.note("lint_input_salt");
            None
        }
        fn format_config<'a>(&self, _src: &SourceFile, cfg: &'a EngineConfig) -> Cow<'a, EngineConfig> {
            self.note("format_config");
            Cow::Borrowed(cfg)
        }
        fn format_input_salt(&self, _path: &std::path::Path, _cfg: &EngineConfig) -> Option<String> {
            self.note("format_input_salt");
            None
        }
        fn lint(&self, _src: &SourceFile, _cfg: &EngineConfig) -> anyhow::Result<Vec<Diagnostic>> {
            self.note("lint");
            Ok(Vec::new())
        }
        fn format(&self, _src: &SourceFile, _cfg: &EngineConfig) -> anyhow::Result<FormatOutput> {
            self.note("format");
            Ok(FormatOutput::Unchanged)
        }
        fn lint_cache_salt(&self, _src: &SourceFile) -> Option<String> {
            self.note("lint_cache_salt");
            None
        }
        fn lint_cache_version(&self, _src: &SourceFile) -> Cow<'_, str> {
            self.note("lint_cache_version");
            Cow::Borrowed("recorder")
        }
        fn batching(&self) -> Option<&dyn Batching> {
            self.note("batching");
            Some(self)
        }
        fn project_analysis(&self) -> Option<&dyn ProjectAnalysis> {
            self.note("project_analysis");
            Some(self)
        }
        fn plan_policy(&self) -> Option<&dyn PlanPolicy> {
            self.note("plan_policy");
            Some(self)
        }
        fn catalog_takeover(&self) -> Option<&dyn CatalogTakeover> {
            self.note("catalog_takeover");
            Some(self)
        }
    }

    impl Batching for Recorder {
        fn batch_support(&self, _cfg: &EngineConfig) -> BatchSupport {
            self.note("batch_support");
            BatchSupport::default()
        }
        fn format_batch(
            &self,
            batch: &[SourceFile],
            _cfg: &EngineConfig,
        ) -> anyhow::Result<Vec<anyhow::Result<FormatOutput>>> {
            self.note("format_batch");
            Ok(batch.iter().map(|_| Ok(FormatOutput::Unchanged)).collect())
        }
        fn lint_batch(
            &self,
            batch: &[SourceFile],
            _cfg: &EngineConfig,
        ) -> anyhow::Result<Vec<anyhow::Result<Vec<Diagnostic>>>> {
            self.note("lint_batch");
            Ok(batch.iter().map(|_| Ok(Vec::new())).collect())
        }
        fn batch_shard_floor(&self) -> usize {
            self.note("batch_shard_floor");
            2
        }
    }

    impl ProjectAnalysis for Recorder {
        fn project_check(
            &self,
            _req: &ProjectRequest<'_>,
        ) -> anyhow::Result<Vec<(std::path::PathBuf, Vec<Diagnostic>)>> {
            self.note("project_check");
            Ok(Vec::new())
        }
        fn project_inputs(&self) -> ProjectInputs {
            self.note("project_inputs");
            ProjectInputs::default()
        }
        fn selection(&self) -> Option<WireSelection> {
            self.note("selection");
            None
        }
    }

    impl PlanPolicy for Recorder {
        fn provides_language_lint(&self, _language: &Language, _cfg: &EngineConfig) -> bool {
            self.note("provides_language_lint");
            true
        }
        fn provides_language_format(&self, _language: &Language, _cfg: &EngineConfig) -> bool {
            self.note("provides_language_format");
            true
        }
        fn missing_tool(&self, _language: &Language, _cfg: &EngineConfig) -> Option<&'static str> {
            self.note("missing_tool");
            None
        }
        fn self_manages_enabled(&self) -> bool {
            self.note("self_manages_enabled");
            false
        }
        fn supersedes_generic_formatter(&self) -> bool {
            self.note("supersedes_generic_formatter");
            false
        }
    }

    impl CatalogTakeover for Recorder {
        fn takes_over_tier_one(&self) -> bool {
            self.note("takes_over_tier_one");
            false
        }
        fn claimed_paths(&self) -> Vec<String> {
            self.note("claimed_paths");
            Vec::new()
        }
        fn supplements_tier_one(&self) -> bool {
            self.note("supplements_tier_one");
            false
        }
    }

    /// Calls the named trait or facet method on `engine`; a name `ENGINE_METHODS` gains without an arm here
    /// fails the test below, so a new method cannot be left unchecked.
    fn call(engine: &dyn Engine, method: &str) {
        let path = std::path::Path::new("a.toml");
        let src = source("a.toml");
        let cfg = cfg();
        match method {
            "name" => {
                let _ = engine.name();
            }
            "languages" => {
                let _ = engine.languages();
            }
            "capabilities" => {
                let _ = engine.capabilities();
            }
            "option_keys" => {
                let _ = engine.option_keys(OptionTable::Format);
            }
            "overridden_hooks" => {
                let _ = engine.overridden_hooks();
            }
            "version" => {
                let _ = engine.version();
            }
            "run_context" => {
                let _ = engine.run_context(&cfg);
            }
            "file_context" => {
                let _ = engine.file_context(path, &cfg);
            }
            "host_hook" => {
                let _ = engine.host_hook();
            }
            "provides_language_lint" => {
                let _ = engine.provides_language_lint(&Language::Toml, &cfg);
            }
            "provides_language_format" => {
                let _ = engine.provides_language_format(&Language::Toml, &cfg);
            }
            "skip_reason" => {
                let _ = engine.skip_reason(&src);
            }
            "lint_input_salt" => {
                let _ = engine.lint_input_salt(path, &cfg);
            }
            "format_config" => {
                let _ = engine.format_config(&src, &cfg);
            }
            "format_input_salt" => {
                let _ = engine.format_input_salt(path, &cfg);
            }
            "lint" => {
                let _ = engine.lint(&src, &cfg);
            }
            "project_check" => {
                let environment = crate::engine::ProjectEnvironment::default();
                let request = ProjectRequest {
                    root: path,
                    files: &[],
                    config: &cfg,
                    environment: &environment,
                    threads: None,
                    targets: None,
                    cache: None,
                };
                let _ = engine.project_check(&request);
            }
            "project_inputs" => {
                let _ = engine.project_inputs();
            }
            "format" => {
                let _ = engine.format(&src, &cfg);
            }
            "batch_support" => {
                let _ = engine.batch_support(&cfg);
            }
            "format_batch" => {
                let _ = engine.format_batch(std::slice::from_ref(&src), &cfg);
            }
            "lint_batch" => {
                let _ = engine.lint_batch(std::slice::from_ref(&src), &cfg);
            }
            "lint_cache_salt" => {
                let _ = engine.lint_cache_salt(&src);
            }
            "lint_cache_version" => {
                let _ = engine.lint_cache_version(&src);
            }
            "batch_shard_floor" => {
                let _ = engine.batch_shard_floor();
            }
            "selection" => {
                let _ = engine.selection();
            }
            "takes_over_tier_one" => {
                let _ = engine.takes_over_tier_one();
            }
            "claimed_paths" => {
                let _ = engine.claimed_paths();
            }
            "supplements_tier_one" => {
                let _ = engine.supplements_tier_one();
            }
            "missing_tool" => {
                let _ = engine.missing_tool(&Language::Toml, &cfg);
            }
            "self_manages_enabled" => {
                let _ = engine.self_manages_enabled();
            }
            "supersedes_generic_formatter" => {
                let _ = engine.supersedes_generic_formatter();
            }
            "batching" => {
                let _ = engine.batching();
            }
            "project_analysis" => {
                let _ = engine.project_analysis();
            }
            "plan_policy" => {
                let _ = engine.plan_policy();
            }
            "catalog_takeover" => {
                let _ = engine.catalog_takeover();
            }
            other => panic!("Engine method `{other}` has no call in this test; add it here and to Recorder"),
        }
    }

    #[test]
    fn every_engine_method_reaches_the_wrapped_engine() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let engine = FormatScope::new(
            Box::new(Recorder(Arc::clone(&calls))),
            GlobSetBuilder::new().build().expect("set"),
        );
        let methods: Vec<&str> = poly_component_protocol::coverage::ENGINE_METHODS
            .iter()
            .map(|(name, _)| *name)
            .collect();
        assert!(
            methods.len() > 20,
            "the method table must not be empty: {}",
            methods.len()
        );
        for method in &methods {
            calls.lock().expect("lock").clear();
            call(&engine, method);
            let seen = calls.lock().expect("lock").clone();
            assert!(
                seen.contains(method),
                "FormatScope does not forward `{method}` to the wrapped engine (saw {seen:?})"
            );
        }
    }

    #[test]
    fn identity_is_the_wrapped_engines() {
        let engine = scoped();
        assert_eq!(engine.name(), "upper");
        assert_eq!(engine.version(), "test");
        assert!(engine.capabilities().format);
    }
}
