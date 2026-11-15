//! How each upstream SDK reads the `permission_suggestions` of a `can_use_tool` request.
//!
//! `sdk.mjs` hands the wire value to `canUseTool` without looking at it. `claude_agent_sdk` runs
//! `PermissionUpdate.from_dict` on every entry, so a broken entry raises inside its control handler and the
//! request is answered with the `str()` of that exception.

use serde_json::Value;

use crate::{JsonObject, PermissionUpdate};

/// The typed view of what `sdk.mjs` passes on: every object entry of an array, decoded when it is a
/// well-formed update and kept verbatim as [`PermissionUpdate::Unknown`] otherwise.
pub(crate) fn typescript(entries: &[Value]) -> Vec<PermissionUpdate> {
    entries
        .iter()
        .filter_map(Value::as_object)
        .cloned()
        .map(typed)
        .collect()
}

/// Python `[PermissionUpdate.from_dict(s) for s in (value or [])]`, each update holding exactly the fields its
/// `to_dict` writes back, or the message of the exception Python raises.
pub(crate) fn python(value: Option<&Value>) -> Result<Vec<PermissionUpdate>, String> {
    let Some(value) = value.filter(|value| truthy(value)) else {
        return Ok(Vec::new());
    };
    iterate(value)?
        .into_iter()
        .map(|entry| match entry {
            Item::Value(Value::Object(object)) => from_dict(object).map(typed),
            other => Err(format!("'{}' object has no attribute 'get'", other.type_name())),
        })
        .collect()
}

fn typed(object: JsonObject) -> PermissionUpdate {
    PermissionUpdate::from_wire_json(object.clone()).unwrap_or(PermissionUpdate::Unknown(object))
}

/// One element a Python `for` loop yields over a decoded JSON value.
enum Item<'a> {
    Value(&'a Value),
    /// A character of a string or a key of a dict: always a `str`.
    Text,
}

impl Item<'_> {
    fn type_name(&self) -> &'static str {
        match self {
            Self::Value(value) => type_name(value),
            Self::Text => "str",
        }
    }
}

fn iterate(value: &Value) -> Result<Vec<Item<'_>>, String> {
    match value {
        Value::Array(entries) => Ok(entries.iter().map(Item::Value).collect()),
        Value::String(text) => Ok(text.chars().map(|_| Item::Text).collect()),
        Value::Object(object) => Ok(object.keys().map(|_| Item::Text).collect()),
        other => Err(format!("'{}' object is not iterable", type_name(other))),
    }
}

/// The Python class name `json.loads` gives a value.
fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(entries) => !entries.is_empty(),
        Value::Object(object) => !object.is_empty(),
    }
}

/// `data.get(key)` that is not `None`.
fn present<'a>(object: &'a JsonObject, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

/// `PermissionUpdate.from_dict(data).to_dict()`.
fn from_dict(data: &JsonObject) -> Result<JsonObject, String> {
    let rules = match present(data, "rules") {
        None => None,
        Some(rules) => Some(iterate(rules)?.into_iter().map(rule).collect::<Result<Vec<_>, _>>()?),
    };
    let kind = data.get("type").ok_or_else(|| "'type'".to_owned())?;
    let mut update = JsonObject::new();
    update.insert("type".to_owned(), kind.clone());
    let copy = |update: &mut JsonObject, key: &str| {
        if let Some(value) = present(data, key) {
            update.insert(key.to_owned(), value.clone());
        }
    };
    copy(&mut update, "destination");
    match kind.as_str() {
        Some("addRules" | "replaceRules" | "removeRules") => {
            if let Some(rules) = rules {
                update.insert("rules".to_owned(), Value::Array(rules));
            }
            copy(&mut update, "behavior");
        }
        Some("setMode") => copy(&mut update, "mode"),
        Some("addDirectories" | "removeDirectories") => copy(&mut update, "directories"),
        _ => {}
    }
    Ok(update)
}

/// `PermissionRuleValue(tool_name=r["toolName"], rule_content=r.get("ruleContent"))` as `to_dict` writes it.
fn rule(item: Item<'_>) -> Result<Value, String> {
    let rule = match item {
        Item::Value(Value::Object(rule)) => rule,
        Item::Text | Item::Value(Value::String(_)) => {
            return Err("string indices must be integers, not 'str'".to_owned());
        }
        Item::Value(Value::Array(_)) => return Err("list indices must be integers or slices, not str".to_owned()),
        Item::Value(other) => return Err(format!("'{}' object is not subscriptable", type_name(other))),
    };
    let tool_name = rule.get("toolName").ok_or_else(|| "'toolName'".to_owned())?;
    let mut value = JsonObject::new();
    value.insert("toolName".to_owned(), tool_name.clone());
    value.insert(
        "ruleContent".to_owned(),
        rule.get("ruleContent").cloned().unwrap_or(Value::Null),
    );
    Ok(Value::Object(value))
}

#[cfg(test)]
#[path = "permission_suggestions_tests.rs"]
mod tests;
