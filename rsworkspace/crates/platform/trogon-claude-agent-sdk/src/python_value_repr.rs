//! Python `repr` and `type(value).__name__` of the value `json.loads` reads from JSON text.

use crate::{
    mcp_wire_ids::marker_text,
    python_json::{float_repr, python_repr},
};
use serde_json::Value;

/// Python `repr` of the value `json.loads` reads from this JSON: `None`, `True`, `False`, single-quoted strings,
/// `{key: value}` mappings in insertion order and floats spelled as Python spells them. A request id too big for
/// 64 bits stands in the message as a marker string and is spelled as the integer Python read.
pub(crate) fn repr(value: &Value) -> String {
    if let Some(integer) = marker_text(value) {
        return integer;
    }
    match value {
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::Number(number) => number
            .as_i64()
            .map(|integer| integer.to_string())
            .or_else(|| number.as_u64().map(|integer| integer.to_string()))
            .or_else(|| number.as_f64().map(float_repr))
            .unwrap_or_else(|| number.to_string()),
        Value::String(text) => python_repr(text),
        Value::Array(items) => format!("[{}]", items.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(object) => format!(
            "{{{}}}",
            object
                .iter()
                .map(|(key, value)| format!("{}: {}", python_repr(key), repr(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `type(value).__name__` of the value `json.loads` reads from this JSON.
pub(crate) fn type_name(value: &Value) -> &'static str {
    if marker_text(value).is_some() {
        return "int";
    }
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

#[cfg(test)]
#[path = "python_value_repr_tests.rs"]
mod tests;
