use std::ffi::OsString;

use tracing::info;

/// Names of the environment variables poly-hooks reads, plus lookups that
/// honour the pre-commit spelling of the ones poly inherited from it.
pub(crate) struct EnvVars;

impl EnvVars {
    pub(crate) const HOME: &'static str = "HOME";
    pub(crate) const CARGO_HOME: &'static str = "CARGO_HOME";

    pub(crate) const PREK_COLOR: &'static str = "PREK_COLOR";
    pub(crate) const PREK_NO_CONCURRENCY: &'static str = "PREK_NO_CONCURRENCY";
    pub(crate) const PREK_MAX_CONCURRENCY: &'static str = "PREK_MAX_CONCURRENCY";
    pub(crate) const PREK_LOG_TRUNCATE_LIMIT: &'static str = "PREK_LOG_TRUNCATE_LIMIT";

    const PRE_COMMIT_NO_CONCURRENCY: &'static str = "PRE_COMMIT_NO_CONCURRENCY";
}

impl EnvVars {
    /// Read an environment variable, falling back to pre-commit corresponding variable if not found.
    pub(crate) fn var_os(name: &str) -> Option<OsString> {
        #[allow(clippy::disallowed_methods)]
        std::env::var_os(name).or_else(|| {
            let name = Self::pre_commit_name(name)?;
            let val = std::env::var_os(name)?;
            info!("Falling back to pre-commit environment variable for {name}");
            Some(val)
        })
    }

    pub(crate) fn is_set(name: &str) -> bool {
        Self::var_os(name).is_some()
    }

    /// Read an environment variable, falling back to pre-commit corresponding variable if not found.
    pub(crate) fn var(name: &str) -> Result<String, std::env::VarError> {
        match Self::var_os(name) {
            Some(s) => s.into_string().map_err(std::env::VarError::NotUnicode),
            None => Err(std::env::VarError::NotPresent),
        }
    }

    /// Read an environment var and parse as bool.
    pub(crate) fn var_as_bool(name: &str) -> Option<bool> {
        if let Some(val) = EnvVars::var_os(name)
            && let Some(val) = val.to_str()
            && let Some(val) = EnvVars::parse_boolish(val)
        {
            Some(val)
        } else {
            None
        }
    }

    /// Parse a boolean from a string.
    ///
    /// Adapted from Clap's `BoolishValueParser` which is dual licensed under the MIT and Apache-2.0.
    /// See `clap_builder/src/util/str_to_bool.rs`
    fn parse_boolish(val: &str) -> Option<bool> {
        const TRUE_LITERALS: [&str; 6] = ["y", "yes", "t", "true", "on", "1"];

        const FALSE_LITERALS: [&str; 6] = ["n", "no", "f", "false", "off", "0"];

        let val = val.to_lowercase();
        let pat = val.as_str();
        if TRUE_LITERALS.contains(&pat) {
            Some(true)
        } else if FALSE_LITERALS.contains(&pat) {
            Some(false)
        } else {
            None
        }
    }

    fn pre_commit_name(name: &str) -> Option<&str> {
        match name {
            Self::PREK_NO_CONCURRENCY => Some(Self::PRE_COMMIT_NO_CONCURRENCY),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EnvVars;

    #[test]
    fn test_parse_boolish() {
        let true_values = ["y", "yes", "t", "true", "on", "1"];
        let false_values = ["n", "no", "f", "false", "off", "0"];
        for val in true_values {
            assert_eq!(EnvVars::parse_boolish(val), Some(true),);
            assert_eq!(EnvVars::parse_boolish(&val.to_uppercase()), Some(true),);
        }
        for val in false_values {
            assert_eq!(EnvVars::parse_boolish(val), Some(false),);
            assert_eq!(EnvVars::parse_boolish(&val.to_uppercase()), Some(false),);
        }
        assert_eq!(EnvVars::parse_boolish("maybe"), None);
        assert_eq!(EnvVars::parse_boolish(""), None);
        assert_eq!(EnvVars::parse_boolish("123"), None);
    }
}
