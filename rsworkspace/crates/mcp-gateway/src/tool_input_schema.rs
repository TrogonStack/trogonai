use serde_json::Value;
use std::fmt;

/// JSON Schema describing an MCP tool's input parameters.
///
/// Wraps a `serde_json::Value` that MUST be a JSON object (the `dict` shape
/// AGT's `MCPSecurityScanner.scan_tool()` expects). The workspace's
/// `serde_json` pin has no `preserve_order` feature, so `Value::Object` is
/// backed by a `BTreeMap` and serializes with keys in sorted order; this is
/// what makes fingerprinting deterministic without a custom canonicalizer.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolInputSchema(Value);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ToolInputSchemaError {
    #[error("tool input schema must be a JSON object, got {0}")]
    NotAnObject(&'static str),
}

impl ToolInputSchema {
    pub fn new(value: Value) -> Result<Self, ToolInputSchemaError> {
        if !value.is_object() {
            return Err(ToolInputSchemaError::NotAnObject(json_type_name(&value)));
        }
        Ok(Self(value))
    }

    /// Canonical serialization used for fingerprinting: object keys are
    /// already sorted by `serde_json::Map`'s underlying `BTreeMap`.
    pub fn canonical_json(&self) -> String {
        // `Value::Object` serialization never fails.
        #[allow(clippy::unwrap_used)]
        serde_json::to_string(&self.0).unwrap_or_default()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn properties(&self) -> Option<&serde_json::Map<String, Value>> {
        self.0.get("properties").and_then(Value::as_object)
    }

    pub fn required(&self) -> Vec<&str> {
        self.0
            .get("required")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    }

    pub fn type_name(&self) -> Option<&str> {
        self.0.get("type").and_then(Value::as_str)
    }

    pub fn additional_properties(&self) -> Option<&Value> {
        self.0.get("additionalProperties")
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

impl fmt::Display for ToolInputSchema {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.canonical_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_non_object() {
        let err = ToolInputSchema::new(json!("not-an-object")).unwrap_err();
        assert_eq!(err, ToolInputSchemaError::NotAnObject("string"));
    }

    #[test]
    fn accepts_object() {
        let schema = ToolInputSchema::new(json!({"type": "object"})).unwrap();
        assert_eq!(schema.type_name(), Some("object"));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let schema = ToolInputSchema::new(json!({"b": 1, "a": 2})).unwrap();
        assert_eq!(schema.canonical_json(), r#"{"a":2,"b":1}"#);
    }

    #[test]
    fn required_reads_string_array() {
        let schema = ToolInputSchema::new(json!({"type": "object", "required": ["a", "b"]})).unwrap();
        assert_eq!(schema.required(), vec!["a", "b"]);
    }
}
