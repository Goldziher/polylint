//! The options of `[engines.tsrs]`, validated.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use anyhow::Context;
use poly_engine_api::engine::OptionType;

/// The project directory, relative to the project root.
pub(crate) const DEFAULT_DIR: &str = ".";

/// Every option, with the type it is read as.
pub(crate) const OPTION_KEYS: &[(&str, OptionType)] = &[
    ("enabled", OptionType::BOOLEAN),
    ("project", OptionType::STRING),
    ("dir", OptionType::STRING),
    ("strict", OptionType::BOOLEAN),
    ("skip_lib_check", OptionType::BOOLEAN),
    ("report_unresolved_modules", OptionType::BOOLEAN),
    ("threads", OptionType::INTEGER),
];

pub(crate) struct Settings {
    /// The directory the `tsconfig.json` is looked for from, relative to the project root.
    pub dir: PathBuf,
    /// The `tsconfig` to check, relative to the project root; the nearest `tsconfig.json` when absent.
    pub project: Option<PathBuf>,
    pub strict: Option<bool>,
    pub skip_lib_check: Option<bool>,
    pub report_unresolved_modules: bool,
    /// The checkers tsrs runs: the option, else the host's job budget, else the machine's parallelism.
    pub threads: NonZeroUsize,
}

impl Settings {
    pub(crate) fn is_enabled(options: &toml::Table) -> bool {
        options.get("enabled").and_then(toml::Value::as_bool) == Some(true)
    }

    pub(crate) fn from_options(options: &toml::Table, budget: Option<usize>) -> anyhow::Result<Self> {
        let dir = string("dir", options.get("dir"))?.map_or_else(|| PathBuf::from(DEFAULT_DIR), PathBuf::from);
        let threads = match options.get("threads") {
            None => budget
                .and_then(NonZeroUsize::new)
                .or_else(|| std::thread::available_parallelism().ok())
                .unwrap_or(NonZeroUsize::MIN),
            Some(value) => value
                .as_integer()
                .and_then(|count| usize::try_from(count).ok())
                .and_then(NonZeroUsize::new)
                .with_context(|| wrong("threads", "a positive integer", value))?,
        };
        Ok(Self {
            dir,
            project: string("project", options.get("project"))?.map(PathBuf::from),
            strict: flag("strict", options.get("strict"))?,
            skip_lib_check: flag("skip_lib_check", options.get("skip_lib_check"))?,
            report_unresolved_modules: flag("report_unresolved_modules", options.get("report_unresolved_modules"))?
                .unwrap_or(false),
            threads,
        })
    }
}

fn wrong(key: &str, expected: &str, found: &toml::Value) -> String {
    format!("option `{key}` must be {expected}, found {}", found.type_str())
}

fn flag(key: &str, value: Option<&toml::Value>) -> anyhow::Result<Option<bool>> {
    value
        .map(|value| value.as_bool().with_context(|| wrong(key, "a boolean", value)))
        .transpose()
}

fn string(key: &str, value: Option<&toml::Value>) -> anyhow::Result<Option<String>> {
    value
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .with_context(|| wrong(key, "a string", value))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str, budget: Option<usize>) -> anyhow::Result<Settings> {
        Settings::from_options(&toml::from_str(text).expect("toml"), budget)
    }

    #[test]
    fn defaults() {
        let settings = parse("", None).expect("defaults");
        assert_eq!(settings.dir, PathBuf::from("."));
        assert_eq!(settings.project, None);
        assert_eq!((settings.strict, settings.skip_lib_check), (None, None));
        assert!(!settings.report_unresolved_modules);
        assert_eq!(
            settings.threads,
            std::thread::available_parallelism().expect("parallelism")
        );
    }

    #[test]
    fn threads_default_to_the_hosts_budget_and_an_option_overrides_it() {
        assert_eq!(parse("", Some(3)).expect("budget").threads.get(), 3);
        assert_eq!(
            parse("", Some(0)).expect("zero is no budget").threads,
            parse("", None).expect("d").threads
        );
        assert_eq!(
            parse("threads = 1", Some(3)).expect("option").threads,
            NonZeroUsize::MIN
        );
    }

    #[test]
    fn wrong_types_are_errors_naming_the_key() {
        for (text, key) in [
            ("project = 1", "project"),
            ("dir = true", "dir"),
            ("strict = \"yes\"", "strict"),
            ("skip_lib_check = 1", "skip_lib_check"),
            ("report_unresolved_modules = []", "report_unresolved_modules"),
            ("threads = 0", "threads"),
            ("threads = \"2\"", "threads"),
        ] {
            let error = parse(text, None)
                .err()
                .unwrap_or_else(|| panic!("`{text}` is accepted"));
            assert!(format!("{error:#}").contains(key), "`{text}`: {error:#}");
        }
    }
}
