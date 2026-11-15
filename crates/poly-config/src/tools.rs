//! `[tools]` — enabling and configuring vendored catalog tools (ADR 0013).
//!
//! poly ships a vendored catalog of standalone CLI tools (`poly-catalog`); the
//! `[tools]` table lets a user opt any of them in from `poly.toml` and tune how
//! it runs. Every entry is keyed by the tool's **catalog name** (e.g. `shfmt`,
//! `clang-format`):
//!
//! ```toml
//! [tools.shfmt]
//! enabled = true              # off by default — presence does not enable
//! command = "format"          # which catalog command to use ("" default)
//! args = ["-i", "2"]          # REPLACE the catalog command's argv entirely
//! stages = ["pre-commit"]     # git stages this tool runs in
//! files = "**/*.sh"           # include glob(s)
//! exclude = "**/vendor/**"    # exclude glob(s)
//! root = "packages/go"        # run from this subdirectory (relative to config root)
//! extra_args = ["-ignore", "SC2129"]  # appended to the argv, before the file list
//! format_extra_args = []      # ... only when the tool formats
//! lint_extra_args = []        # ... only when the tool lints
//! bin = "/opt/tools/shfmt"    # use this executable instead of the one on PATH
//! version = "3.12.0"          # "latest" (default) or an exact upstream version
//! install = "auto"            # download the pinned release when it is missing
//!
//! [tools.shfmt.env]           # environment variables injected when running the tool
//! GOPATH = "/home/user/go"
//! ```
//!
//! Tools are **off by default**: a bare `[tools.<name>]` table with no
//! `enabled = true` is parsed but inert. Tool names are validated against
//! [`poly_catalog::Catalog`] via [`ToolsConfig::validate`] — an unknown name is
//! rejected (with a closest-match suggestion when one is near).

use std::collections::BTreeMap;

use poly_catalog::Catalog;
use serde::Deserialize;

use crate::hooks::{Patterns, Stage};

/// What poly does when a tool's executable cannot be found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstallPolicy {
    /// Say so, with the command that installs it. The default: poly never
    /// downloads an executable the config did not ask for.
    #[default]
    Never,
    /// Download the pinned release ([`ToolConfig::version`]) into poly's tool
    /// cache, verify its SHA-256, and use it.
    Auto,
}

/// `[tools]` — a map from catalog tool name to its [`ToolConfig`].
///
/// Deserializes transparently from the `[tools.<name>]` tables. Empty by
/// default (no tools enabled).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct ToolsConfig(BTreeMap<String, ToolConfig>);

impl ToolsConfig {
    /// Look up a tool's configuration by its catalog name.
    pub fn get(&self, name: &str) -> Option<&ToolConfig> {
        self.0.get(name)
    }

    /// Iterate over the `(name, config)` pairs.
    pub fn iter(&self) -> std::collections::btree_map::Iter<'_, String, ToolConfig> {
        self.0.iter()
    }

    /// Whether any tool is configured.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Number of configured tools.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Validate every configured tool name against the vendored
    /// [`poly_catalog::Catalog`] (allowlist), then each tool's
    /// [always-on restrictions](ToolConfig::validate_restrictions).
    ///
    /// Returns a human-readable error for the first name not present in the
    /// catalog, suggesting the closest known name when one is within a small
    /// edit distance.
    pub fn validate(&self) -> Result<(), String> {
        let catalog = Catalog::get();
        for (name, tool) in &self.0 {
            if catalog.tool(name).is_none() {
                return Err(unknown_tool_message(name, catalog));
            }
            tool.validate_restrictions()
                .map_err(|reason| format!("[tools.{name}] {reason}"))?;
        }
        Ok(())
    }
}

impl<'a> IntoIterator for &'a ToolsConfig {
    type Item = (&'a String, &'a ToolConfig);
    type IntoIter = std::collections::btree_map::Iter<'a, String, ToolConfig>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl From<BTreeMap<String, ToolConfig>> for ToolsConfig {
    fn from(map: BTreeMap<String, ToolConfig>) -> Self {
        ToolsConfig(map)
    }
}

/// Configuration for a single catalog tool under `[tools.<name>]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ToolConfig {
    /// Whether this tool is active. **Off by default** — declaring the table
    /// is not enough; `enabled = true` is required.
    pub enabled: bool,
    /// Which catalog command to invoke (`""` is the tool's default command,
    /// `"format"`, `"check"`, …). `None` lets poly pick by intent.
    pub command: Option<String>,
    /// When present, **replaces** the catalog command's argument vector
    /// entirely; when `None`, the catalog command's own argv is used.
    pub args: Option<Vec<String>>,
    /// Git stages this tool runs in; empty means it is not bound to a stage.
    pub stages: Vec<Stage>,
    /// File include glob(s); `None` matches every candidate file.
    pub files: Option<Patterns>,
    /// File exclude glob(s) filtered from the matched set before the tool runs.
    pub exclude: Option<Patterns>,
    /// Environment variables injected on top of the inherited environment when
    /// the tool runs. Applied on both the direct-engine path and the hooks-lowering
    /// path.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Working directory the tool runs in, relative to the config/repo root.
    /// `None` means the repo root. Applied on both the direct-engine path and the
    /// hooks-lowering path.
    pub root: Option<String>,
    /// Arguments added to the tool's argv for every role, immediately before the
    /// file list. Unlike `args` this keeps the catalog's own flags (the ones the
    /// report parser depends on), so it is the place for configuration:
    /// `["-c", "checkstyle.xml", "-p", "checkstyle.properties"]`.
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// `extra_args` for the formatting role only.
    #[serde(default)]
    pub format_extra_args: Vec<String>,
    /// `extra_args` for the linting role only.
    #[serde(default)]
    pub lint_extra_args: Vec<String>,
    /// Executable to run instead of the catalog's `binary` looked up on `PATH`.
    /// For a JVM tool this is the `java` to launch the jar with.
    pub bin: Option<String>,
    /// Release of the tool to use: `"latest"` (the default for a tool poly can
    /// download, when `install = "auto"`) or an exact upstream version such as
    /// `"2.14.1"`. Resolved against the tool's release source; `poly.lock`
    /// records the answer so runs need no lookup.
    pub version: Option<String>,
    /// How long a resolved `latest` is trusted before it is revalidated with
    /// upstream: a number with `s`, `m`, `h` or `d` (default `24h`; `0` always
    /// revalidates).
    pub resolve_ttl: Option<String>,
    /// Download URL for `version`, overriding the release source (with `sha256`).
    pub url: Option<String>,
    /// Lower-case hex SHA-256 the download for `version` must have.
    pub sha256: Option<String>,
    /// Whether poly may download the pinned release when the tool is missing.
    #[serde(default)]
    pub install: InstallPolicy,
    /// Values files passed to every chart (`helm lint -f <file>`), relative to the
    /// config root. Only for tools whose recipe takes values files (helm).
    #[serde(default)]
    pub values_files: Vec<String>,
    /// Values files for particular charts. Each key is a glob matched against the
    /// chart directory relative to the config root (`armis/service_charts/redash`,
    /// `**/redash`); the value lists the files passed for the charts it matches.
    #[serde(default)]
    pub chart_values: BTreeMap<String, Vec<String>>,
    /// Chart directories (globs, relative to the config root) the tool does not
    /// run on at all, e.g. a library chart `helm template` cannot render.
    #[serde(default)]
    pub skip_charts: Vec<String>,
    /// Set to `false` to skip the tool's prepare step (`helm dependency build`,
    /// run once per chart before linting when `charts/` does not satisfy
    /// `Chart.lock`). The step is also skipped when `POLY_OFFLINE` is set.
    pub prepare: Option<bool>,
    /// How many times the prepare step is attempted before the chart is checked
    /// with the dependencies on disk (default 5). `1` disables the retry.
    pub prepare_attempts: Option<u32>,
    /// Seconds to wait after the first failed prepare attempt (default 4); the
    /// wait doubles after each further failure (4, 8, 16, 32).
    pub prepare_backoff_secs: Option<u64>,
    /// Up to this many extra seconds, chosen at random, added to every wait so
    /// concurrent runners do not retry in lockstep (default 4).
    pub prepare_jitter_secs: Option<u64>,
    /// Whether a chart in `skip_charts` is still run, and reported as a failure
    /// when the tool now succeeds on it, so the list cannot outlive its reason
    /// (default `true`). `false` skips such a chart without running it.
    pub verify_skip_charts: Option<bool>,
    /// Report only findings introduced since this git revision
    /// (`golangci-lint --new-from-rev=<rev>`). Only for tools whose recipe has a
    /// new-from-rev flag; unset reports every finding of the package.
    pub new_from_rev: Option<String>,
    /// Operating systems to run the tool for, one invocation per entry with
    /// `GOOS` set (`["linux", "windows"]`); findings are merged. Empty runs once
    /// with the inherited environment.
    #[serde(default)]
    pub goos: Vec<String>,
}

/// Environment variables `[tools.<name>] env` may never set, trusted or not
/// (ADR 0059): each makes the dynamic loader, git, a shell or a language runtime
/// load code the tool itself never asked for. Matched case-insensitively, since
/// Windows environment names are.
const DENIED_ENV: &[&str] = &[
    "BASH_ENV",
    "ENV",
    "GIT_ASKPASS",
    "GIT_EXEC_PATH",
    "GIT_EXTERNAL_DIFF",
    "GIT_PROXY_COMMAND",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "JAVA_TOOL_OPTIONS",
    "JDK_JAVA_OPTIONS",
    "NODE_OPTIONS",
    "PERL5OPT",
    "PYTHONSTARTUP",
    "RUBYOPT",
    "_JAVA_OPTIONS",
];

/// Prefixes of [`DENIED_ENV`]-like families: the Linux and macOS loaders'
/// `LD_*`/`DYLD_*` (preload, library and audit paths) and git's `GIT_CONFIG*`,
/// which injects configuration such as `core.fsmonitor` that runs a command.
const DENIED_ENV_PREFIXES: &[&str] = &["DYLD_", "GIT_CONFIG", "LD_"];

/// Whether `[tools.<name>] env` may not set `name`.
#[must_use]
pub fn env_key_denied(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    DENIED_ENV.contains(&upper.as_str()) || DENIED_ENV_PREFIXES.iter().any(|prefix| upper.starts_with(prefix))
}

impl ToolConfig {
    /// The restrictions that hold whether or not the repository is trusted: `bin`
    /// is a bare program name or an absolute path, never a path relative to
    /// wherever poly runs; `root` stays inside the repository; `env` sets none of
    /// the [`DENIED_ENV`] variables.
    ///
    /// # Errors
    ///
    /// A message naming the first offending key.
    pub fn validate_restrictions(&self) -> Result<(), String> {
        if let Some(bin) = &self.bin
            && !crate::trust::is_bare_program(bin)
            && !std::path::Path::new(bin).is_absolute()
        {
            return Err(format!(
                "`bin = \"{bin}\"` must be a program name looked up on PATH or an absolute path"
            ));
        }
        if let Some(root) = &self.root
            && escapes_repository(root)
        {
            return Err(format!(
                "`root = \"{root}\"` must be a relative path inside the repository, without `..`"
            ));
        }
        if let Some(name) = self.env.keys().find(|name| env_key_denied(name)) {
            return Err(format!(
                "`env` may not set `{name}`: it would make the loader, git or a runtime run code the tool did not \
                 ask for"
            ));
        }
        Ok(())
    }
}

/// Whether `root` names a place outside the repository: rooted, drive-qualified,
/// or climbing with `..`. Inspected as a string so every host gives the same answer.
fn escapes_repository(root: &str) -> bool {
    let mut chars = root.chars();
    let drive = matches!((chars.next(), chars.next()), (Some(letter), Some(':')) if letter.is_ascii_alphabetic());
    root.starts_with(['/', '\\']) || drive || root.split(['/', '\\']).any(|segment| segment == "..")
}

/// Maximum edit distance at which a closest-match name is worth suggesting.
const SUGGESTION_MAX_DISTANCE: usize = 3;

/// Build the "unknown tool" error, appending a closest-match suggestion when a
/// catalog name is within [`SUGGESTION_MAX_DISTANCE`] of `name`.
fn unknown_tool_message(name: &str, catalog: &Catalog) -> String {
    let suggestion = catalog
        .tools()
        .iter()
        .map(|tool| (tool.name.as_str(), levenshtein(name, &tool.name)))
        .filter(|(_, distance)| *distance <= SUGGESTION_MAX_DISTANCE)
        .min_by_key(|(_, distance)| *distance)
        .map(|(candidate, _)| candidate);
    match suggestion {
        Some(candidate) => format!(
            "unknown tool `{name}` in [tools]; did you mean `{candidate}`? \
             (no such tool in the poly catalog)"
        ),
        None => format!("unknown tool `{name}` in [tools]; no such tool in the poly catalog"),
    }
}

/// Classic two-row Levenshtein edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let b_chars: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b_chars.len()).collect();
    let mut current = vec![0usize; b_chars.len() + 1];
    for (i, a_char) in a.chars().enumerate() {
        current[0] = i + 1;
        for (j, &b_char) in b_chars.iter().enumerate() {
            let cost = usize::from(a_char != b_char);
            current[j + 1] = (previous[j + 1] + 1).min(current[j] + 1).min(previous[j] + cost);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b_chars.len()]
}

/// An absolute executable path on the host running the tests; Windows needs a drive.
#[cfg(all(test, windows))]
const ABSOLUTE_BIN: &str = "C:/opt/tools/bin/tool";
/// An absolute executable path on the host running the tests.
#[cfg(all(test, not(windows)))]
const ABSOLUTE_BIN: &str = "/opt/tools/bin/tool";

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> ToolsConfig {
        toml::from_str(toml).expect("parse [tools]")
    }

    #[test]
    fn should_parse_full_and_minimal_tool_entries() {
        let tools = parse(
            r#"
[shfmt]
enabled = true
command = "format"
args = ["-i", "2"]
stages = ["pre-commit"]
files = "**/*.sh"
exclude = "**/vendor/**"

[clang-format]
enabled = true
"#,
        );

        let shfmt = tools.get("shfmt").expect("shfmt entry present");
        assert!(shfmt.enabled);
        assert_eq!(shfmt.command.as_deref(), Some("format"));
        assert_eq!(shfmt.args.as_deref(), Some(&["-i".to_string(), "2".to_string()][..]));
        assert_eq!(shfmt.stages, vec![Stage::PreCommit]);
        assert_eq!(
            shfmt.files.as_ref().map(Patterns::as_slice),
            Some(&["**/*.sh".to_string()][..])
        );
        assert_eq!(
            shfmt.exclude.as_ref().map(Patterns::as_slice),
            Some(&["**/vendor/**".to_string()][..])
        );

        let clang = tools.get("clang-format").expect("clang-format entry present");
        assert!(clang.enabled);
        assert_eq!(clang.command, None);
        assert_eq!(clang.args, None);
        assert!(clang.stages.is_empty());
        assert!(clang.files.is_none());
        assert!(clang.exclude.is_none());

        assert_eq!(tools.len(), 2);
    }

    #[test]
    fn should_default_enabled_to_false_for_bare_table() {
        let tools = parse("[shfmt]\n");
        let shfmt = tools.get("shfmt").expect("shfmt entry present");
        assert!(!shfmt.enabled, "tools are off until enabled = true");
    }

    #[test]
    fn should_capture_args_override_as_some_and_absent_as_none() {
        let tools = parse(
            r#"
[shfmt]
enabled = true
args = ["--flag", "value"]

[gofmt]
enabled = true
"#,
        );
        assert_eq!(
            tools.get("shfmt").unwrap().args.as_deref(),
            Some(&["--flag".to_string(), "value".to_string()][..])
        );
        assert_eq!(tools.get("gofmt").unwrap().args, None);
    }

    #[test]
    fn should_accept_known_catalog_tool_names() {
        let tools = parse(
            r#"
[shfmt]
enabled = true
[clang-format]
enabled = true
"#,
        );
        tools.validate().expect("known tools must validate");
    }

    #[test]
    fn should_reject_unknown_tool_name() {
        let tools = parse("[definitely-not-a-tool]\nenabled = true\n");
        let error = tools.validate().unwrap_err();
        assert!(
            error.contains("definitely-not-a-tool"),
            "names the offending tool: {error}"
        );
        assert!(error.contains("no such tool"), "explains the rejection: {error}");
    }

    #[test]
    fn should_suggest_closest_name_for_near_miss() {
        let tools = parse("[shfmtt]\nenabled = true\n");
        let error = tools.validate().unwrap_err();
        assert!(
            error.contains("did you mean `shfmt`"),
            "suggests the closest catalog name: {error}"
        );
    }

    #[test]
    fn should_reject_unknown_keys_in_tool_table() {
        let result: Result<ToolsConfig, _> = toml::from_str("[shfmt]\nbogus = true\n");
        assert!(result.is_err(), "deny_unknown_fields must reject `bogus`");
    }

    #[test]
    fn empty_tools_config_validates() {
        let tools = ToolsConfig::default();
        assert!(tools.is_empty());
        tools.validate().expect("empty config is valid");
    }

    #[test]
    fn should_parse_env_vars_and_root() {
        let tools = parse(
            r#"
[shfmt]
enabled = true
root = "packages/shell"

[shfmt.env]
MYVAR = "hello"
OTHER = "world"
"#,
        );
        let shfmt = tools.get("shfmt").expect("shfmt entry present");
        assert!(shfmt.enabled);
        assert_eq!(shfmt.root.as_deref(), Some("packages/shell"));
        assert_eq!(shfmt.env.get("MYVAR").map(String::as_str), Some("hello"));
        assert_eq!(shfmt.env.get("OTHER").map(String::as_str), Some("world"));
    }

    #[test]
    fn should_default_env_to_empty_and_root_to_none() {
        let tools = parse("[shfmt]\nenabled = true\n");
        let shfmt = tools.get("shfmt").expect("shfmt entry present");
        assert!(shfmt.env.is_empty(), "env defaults to empty BTreeMap");
        assert!(shfmt.root.is_none(), "root defaults to None");
    }
}

#[cfg(test)]
mod recipe_keys {
    use super::*;

    #[test]
    fn should_parse_extra_args_pins_and_install_policy() {
        let tools: ToolsConfig = toml::from_str(&format!(
            r#"
[checkstyle]
enabled = true
extra_args = ["-c", "checkstyle.xml"]
lint_extra_args = ["-p", "checkstyle.properties"]
version = "12.3.1"
install = "auto"
bin = "{ABSOLUTE_BIN}"

[buildifier]
enabled = true
format_extra_args = ["-lint=fix"]
"#
        ))
        .expect("parse");
        let checkstyle = tools.get("checkstyle").expect("checkstyle");
        assert_eq!(checkstyle.extra_args, ["-c", "checkstyle.xml"]);
        assert_eq!(checkstyle.lint_extra_args, ["-p", "checkstyle.properties"]);
        assert_eq!(checkstyle.version.as_deref(), Some("12.3.1"));
        assert_eq!(checkstyle.install, InstallPolicy::Auto);
        assert_eq!(
            tools.get("buildifier").expect("buildifier").install,
            InstallPolicy::Never
        );
        tools.validate().expect("both are catalog tools");
    }

    #[test]
    fn should_reject_an_unknown_install_policy() {
        let result: Result<ToolsConfig, _> = toml::from_str("[buf]\ninstall = \"sometimes\"\n");
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod restrictions {
    use super::*;

    fn validate(toml: &str) -> Result<(), String> {
        toml::from_str::<ToolsConfig>(toml).expect("parse").validate()
    }

    #[test]
    fn bin_is_a_bare_name_or_an_absolute_path() {
        validate("[shfmt]\nbin = \"shfmt\"\n").expect("a PATH lookup");
        validate(&format!("[shfmt]\nbin = \"{ABSOLUTE_BIN}\"\n")).expect("an absolute path");
        for bin in ["./shfmt", "bin/shfmt", "../shfmt"] {
            let error = validate(&format!("[shfmt]\nbin = \"{bin}\"\n")).expect_err(bin);
            assert!(
                error.contains("[tools.shfmt]") && error.contains("absolute path"),
                "{error}"
            );
        }
    }

    #[test]
    fn root_stays_inside_the_repository() {
        validate("[shfmt]\nroot = \"packages/shell\"\n").expect("a subdirectory");
        for root in [
            "../elsewhere",
            "a/../../b",
            "/etc",
            "C:\\\\x",
            "\\\\\\\\server\\\\share",
        ] {
            let error = validate(&format!("[shfmt]\nroot = \"{root}\"\n")).expect_err(root);
            assert!(error.contains("inside the repository"), "{root}: {error}");
        }
    }

    #[test]
    fn env_may_not_inject_code() {
        validate("[shfmt.env]\nGOFLAGS = \"-mod=mod\"\nPATH = \"/opt/go/bin\"\nJAVA_HOME = \"/opt/jdk\"\n")
            .expect("ordinary variables");
        for name in [
            "LD_PRELOAD",
            "ld_library_path",
            "LD_AUDIT",
            "DYLD_INSERT_LIBRARIES",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_PARAMETERS",
            "GIT_SSH_COMMAND",
            "BASH_ENV",
            "NODE_OPTIONS",
            "JAVA_TOOL_OPTIONS",
        ] {
            let error = validate(&format!("[shfmt.env]\n{name} = \"x\"\n")).expect_err(name);
            assert!(error.contains(name), "{error}");
        }
    }
}
