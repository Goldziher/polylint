//! Checking a tool call's arguments before they are acted on.
//!
//! A key the tool does not take, a value of the wrong type and a parameter that does not apply
//! to the chosen `scope`, `action` or `topics` are all refused with a usage error naming the
//! key. Dropping them instead answers a different question than the one asked: `check {path:
//! [...]}` (a typo for `paths`) would lint the whole directory, and `manage {action:
//! cache_clean, dry_run: true}` would clean the cache.

use rmcp::model::{JsonObject, Tool};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::dto::TextRepr;

/// What the parameters of one selector value (`scope`, `action`, a topic) are.
pub(super) type Applies = &'static [(&'static str, &'static [&'static str])];

/// The parameters each `info` topic reads.
pub(super) const INFO_TOPICS: Applies = &[
    ("version", &[]),
    ("config", &["config"]),
    ("validate", &["paths"]),
    ("rules", &["config", "dirs", "test"]),
    ("components", &["config", "paths", "install"]),
    ("cache", &["config"]),
];

/// The parameters each `manage` action reads.
pub(super) const MANAGE_ACTIONS: Applies = &[
    ("cache_clean", &["config"]),
    ("cache_gc", &["max_age_days", "max_size", "config"]),
    (
        "components_install",
        &["names", "languages", "paths", "config", "dry_run"],
    ),
    ("components_uninstall", &["names"]),
    ("components_prune", &[]),
];

/// The run options the whole-project phase reads; the rest shape the per-file tier.
pub(super) const WORKSPACE_SCOPE: Applies = &[("workspace", &["config", "jobs", "no_cache"])];

/// The text block a caller asked for, read from the raw arguments so that a refusal of the
/// other arguments still honours it.
pub(super) fn text_repr(arguments: &JsonObject) -> TextRepr {
    match arguments.get("format").and_then(Value::as_str) {
        Some("toon") => TextRepr::Toon,
        _ => TextRepr::Json,
    }
}

/// `arguments` as `tool`'s typed parameters.
///
/// # Errors
///
/// The usage message: a key the tool's schema does not list, or a value that does not
/// deserialize.
pub(super) fn parse<T: DeserializeOwned>(tool: &Tool, arguments: &JsonObject) -> Result<T, String> {
    let known: Vec<&str> = tool
        .input_schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().map(String::as_str).collect())
        .unwrap_or_default();
    if let Some(unknown) = arguments.keys().find(|key| !known.contains(&key.as_str())) {
        return Err(format!(
            "unknown argument `{unknown}` for `{}`; it takes: {}",
            tool.name,
            known.join(", ")
        ));
    }
    serde_json::from_value(Value::Object(arguments.clone()))
        .map_err(|error| format!("invalid arguments for `{}`: {error}", tool.name))
}

/// Whether the caller gave `key` a value that asks for something: absent, `null`, `false` and
/// an empty list all leave the default in place.
fn given(arguments: &JsonObject, key: &str) -> bool {
    match arguments.get(key) {
        None | Some(Value::Null | Value::Bool(false)) => false,
        Some(Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

/// Refuse an argument none of the `selected` values of `selector` reads.
///
/// `applies` lists every parameter that depends on the selector; one it does not mention at all
/// applies to every value. `always` are the parameters that do not depend on it.
///
/// # Errors
///
/// The usage message naming the argument and, when another value reads it, which.
pub(super) fn applicable(
    arguments: &JsonObject,
    selector: &str,
    selected: &[&str],
    applies: Applies,
    always: &[&str],
) -> Result<(), String> {
    for key in arguments.keys().map(String::as_str) {
        if key == selector || always.contains(&key) || !given(arguments, key) {
            continue;
        }
        let reads = |name: &str| {
            applies
                .iter()
                .any(|(value, params)| *value == name && params.contains(&key))
        };
        if selected.iter().any(|name| reads(name)) {
            continue;
        }
        let takers: Vec<&str> = applies
            .iter()
            .filter(|(_, params)| params.contains(&key))
            .map(|(value, _)| *value)
            .collect();
        let chosen = selected.join(", ");
        return Err(if takers.is_empty() {
            format!("`{key}` does not apply to `{selector}: {chosen}`")
        } else {
            format!(
                "`{key}` does not apply to `{selector}: {chosen}`; it applies to: {}",
                takers.join(", ")
            )
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(value: Value) -> JsonObject {
        match value {
            Value::Object(object) => object,
            _ => panic!("an object"),
        }
    }

    #[test]
    fn a_default_valued_argument_is_not_given() {
        let arguments = object(serde_json::json!({ "dry_run": false, "names": [], "max_size": null }));
        assert_eq!(
            applicable(&arguments, "action", &["cache_clean"], MANAGE_ACTIONS, &["format"]),
            Ok(())
        );
    }

    #[test]
    fn a_misapplied_argument_names_itself_and_where_it_belongs() {
        let arguments = object(serde_json::json!({ "action": "cache_clean", "dry_run": true }));
        let message = applicable(&arguments, "action", &["cache_clean"], MANAGE_ACTIONS, &["format"]).unwrap_err();
        assert!(
            message.contains("`dry_run`") && message.contains("components_install"),
            "{message}"
        );
    }

    #[test]
    fn an_argument_is_applicable_when_any_selected_value_reads_it() {
        let arguments = object(serde_json::json!({ "paths": ["a"], "dirs": ["r"] }));
        assert_eq!(
            applicable(
                &arguments,
                "topics",
                &["version", "validate", "rules"],
                INFO_TOPICS,
                &["format"]
            ),
            Ok(())
        );
        let message = applicable(&arguments, "topics", &["validate"], INFO_TOPICS, &["format"]).unwrap_err();
        assert!(message.contains("`dirs`") && message.contains("rules"), "{message}");
    }
}
