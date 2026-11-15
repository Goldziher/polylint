//! Typed reads of an engine's TOML options table.
//!
//! Every engine reads `EngineConfig::options` key by key; these accessors replace the
//! `get(key).and_then(toml::Value::as_*)` chains so each read is one call that names its key and type.

/// Typed accessors over a TOML options table. A missing key and a key of the wrong type both read as absent.
pub trait OptionsExt {
    /// The boolean at `key`.
    fn opt_bool(&self, key: &str) -> Option<bool>;
    /// The string at `key`.
    fn opt_str(&self, key: &str) -> Option<&str>;
    /// The integer at `key`.
    fn opt_i64(&self, key: &str) -> Option<i64>;
    /// The integer at `key`, `None` when it does not fit a `u32`.
    fn opt_u32(&self, key: &str) -> Option<u32>;
    /// The integer at `key`, `None` when it does not fit a `usize`.
    fn opt_usize(&self, key: &str) -> Option<usize>;
    /// The strings of the array at `key`; non-string elements are skipped and a non-array reads as empty.
    fn opt_string_list(&self, key: &str) -> Vec<String>;
    /// The sub-table at `key`.
    fn opt_table(&self, key: &str) -> Option<&toml::Table>;
}

impl OptionsExt for toml::Table {
    fn opt_bool(&self, key: &str) -> Option<bool> {
        self.get(key).and_then(toml::Value::as_bool)
    }

    fn opt_str(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(toml::Value::as_str)
    }

    fn opt_i64(&self, key: &str) -> Option<i64> {
        self.get(key).and_then(toml::Value::as_integer)
    }

    fn opt_u32(&self, key: &str) -> Option<u32> {
        self.opt_i64(key).and_then(|n| u32::try_from(n).ok())
    }

    fn opt_usize(&self, key: &str) -> Option<usize> {
        self.opt_i64(key).and_then(|n| usize::try_from(n).ok())
    }

    fn opt_string_list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .and_then(toml::Value::as_array)
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    }

    fn opt_table(&self, key: &str) -> Option<&toml::Table> {
        self.get(key).and_then(toml::Value::as_table)
    }
}

#[cfg(test)]
mod tests {
    use super::OptionsExt;

    fn table() -> toml::Table {
        toml::from_str(
            r#"
            flag = true
            name = "x"
            n = 7
            neg = -1
            big = 5000000000
            list = ["a", 1, "b"]
            [sub]
            k = 1
            "#,
        )
        .expect("valid toml")
    }

    #[test]
    fn reads_scalars_by_type() {
        let t = table();
        assert_eq!(t.opt_bool("flag"), Some(true));
        assert_eq!(t.opt_str("name"), Some("x"));
        assert_eq!(t.opt_i64("n"), Some(7));
    }

    #[test]
    fn wrong_type_or_missing_reads_as_absent() {
        let t = table();
        assert_eq!(t.opt_bool("name"), None);
        assert_eq!(t.opt_str("n"), None);
        assert_eq!(t.opt_i64("missing"), None);
        assert_eq!(t.opt_table("n"), None);
        assert!(t.opt_string_list("name").is_empty());
    }

    #[test]
    fn narrowing_is_checked() {
        let t = table();
        assert_eq!(t.opt_u32("n"), Some(7));
        assert_eq!(t.opt_usize("n"), Some(7));
        assert_eq!(t.opt_u32("neg"), None);
        assert_eq!(t.opt_usize("neg"), None);
        #[cfg(target_pointer_width = "64")]
        assert_eq!(t.opt_u32("big"), None);
        assert_eq!(t.opt_u32("flag"), None);
    }

    #[test]
    fn string_list_skips_non_strings() {
        assert_eq!(table().opt_string_list("list"), vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn table_reads_sub_table() {
        let t = table();
        assert_eq!(t.opt_table("sub").and_then(|s| s.opt_i64("k")), Some(1));
    }
}
