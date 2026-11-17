//! Compacting a tool's input schema for the model.
//!
//! Every tool listing is sent to the model, so its schema is paid for in tokens on every
//! session. schemars renders a unit enum as a `oneOf` of documented constants behind a `$ref`,
//! and states trivial defaults (`[]`, `false`, `null`). [`compact`] inlines the references,
//! turns each enum into a plain `enum` list, drops the trivial defaults, the null alternative of
//! an optional field, integer widths and the description of a described list's members, and
//! unwraps descriptions; the field descriptions already say what each value means.

use std::sync::Arc;

use rmcp::model::JsonObject;
use serde_json::{Map, Value};

/// `schema` with its `$defs` inlined, its enums plain and its trivial defaults removed.
#[must_use]
pub fn compact(schema: &JsonObject) -> Arc<JsonObject> {
    let mut root = schema.clone();
    let defs = match root.remove("$defs") {
        Some(Value::Object(defs)) => defs,
        _ => Map::new(),
    };
    root.remove("$schema");
    root.remove("title");
    let mut value = Value::Object(root);
    rewrite(&mut value, &defs);
    match value {
        Value::Object(object) => Arc::new(object),
        _ => unreachable!("a rewritten object stays an object"),
    }
}

fn rewrite(value: &mut Value, defs: &Map<String, Value>) {
    match value {
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get("$ref")
                && let Some(target) = reference.strip_prefix("#/$defs/").and_then(|name| defs.get(name))
            {
                let mut inlined = target.clone();
                if let (Value::Object(inlined), Some(Value::String(description))) =
                    (&mut inlined, object.get("description"))
                {
                    inlined.insert("description".to_owned(), Value::String(description.clone()));
                }
                *value = inlined;
                rewrite(value, defs);
                return;
            }
            if let Some(only) = optional_branch(object) {
                let description = object.remove("description");
                *value = only;
                if let (Value::Object(inner), Some(description)) = (&mut *value, description) {
                    inner.insert("description".to_owned(), description);
                }
                rewrite(value, defs);
                return;
            }
            object.remove("title");
            // An integer's width and sign say nothing a caller acts on.
            if object
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|format| format.starts_with("uint"))
            {
                object.remove("format");
                object.remove("minimum");
            }
            // `["string", "null"]`: the field may be omitted, so only the real type is kept.
            if let Some(Value::Array(types)) = object.get("type")
                && let [only] = types
                    .iter()
                    .filter(|kind| kind.as_str() != Some("null"))
                    .collect::<Vec<_>>()
                    .as_slice()
            {
                let only = (*only).clone();
                object.insert("type".to_owned(), only);
            }
            // Rustdoc line breaks are not paragraph breaks.
            if let Some(Value::String(description)) = object.get_mut("description") {
                *description = description.split_whitespace().collect::<Vec<_>>().join(" ");
            }
            if object.get("default").is_some_and(is_trivial) {
                object.remove("default");
            }
            collapse_enum(object);
            for child in object.values_mut() {
                rewrite(child, defs);
            }
            // A list's description says what its members are; the member type's own does not add to it.
            if object.contains_key("description")
                && let Some(Value::Object(items)) = object.get_mut("items")
            {
                items.remove("description");
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite(item, defs);
            }
        }
        _ => {}
    }
}

/// The one non-null branch of an `anyOf: [X, {"type": "null"}]`, the shape an `Option` field takes.
/// The field may simply be omitted, so the null branch adds nothing a caller needs.
fn optional_branch(object: &Map<String, Value>) -> Option<Value> {
    let Some(Value::Array(branches)) = object.get("anyOf") else {
        return None;
    };
    let null = serde_json::json!({ "type": "null" });
    match branches.as_slice() {
        [only, other] | [other, only] if *other == null && *only != null => Some(only.clone()),
        _ => None,
    }
}

/// Turn a `oneOf` whose every branch is a single constant into `{"type": "string", "enum": [...]}`.
fn collapse_enum(object: &mut Map<String, Value>) {
    let Some(Value::Array(branches)) = object.get("oneOf") else {
        return;
    };
    let constants: Option<Vec<Value>> = branches.iter().map(|branch| branch.get("const").cloned()).collect();
    let Some(constants) = constants else {
        return;
    };
    object.remove("oneOf");
    object.insert("type".to_owned(), Value::String("string".to_owned()));
    object.insert("enum".to_owned(), Value::Array(constants));
}

fn is_trivial(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => true,
        Value::Array(items) => items.is_empty(),
        Value::String(text) => text.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_are_inlined_and_enums_made_plain() {
        let schema = serde_json::json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "title": "Params",
            "type": "object",
            "properties": {
                "format": { "$ref": "#/$defs/Repr", "description": "Text block format." },
                "paths": { "type": "array", "items": { "type": "string" }, "default": [] },
                "kind": { "type": "string", "default": "lint" },
            },
            "$defs": {
                "Repr": {
                    "oneOf": [
                        { "const": "json", "description": "JSON." },
                        { "const": "toon", "description": "TOON." },
                    ]
                }
            }
        });
        let Value::Object(schema) = schema else { unreachable!() };
        let compacted = Value::Object(compact(&schema).as_ref().clone());
        assert_eq!(
            compacted,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "format": { "type": "string", "enum": ["json", "toon"], "description": "Text block format." },
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "kind": { "type": "string", "default": "lint" },
                },
            })
        );
    }
}
