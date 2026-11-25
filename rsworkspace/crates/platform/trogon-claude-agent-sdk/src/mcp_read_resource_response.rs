use serde_json::Value;

/// The `response` field of an `mcp_read_resource` reply as Claude Code sent it.
///
/// TypeScript `readMcpResource` returns that field without checking its shape, so the value is kept as sent,
/// `null` and non-object values included, and the documented fields are read through accessors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct McpReadResourceResponse {
    response: Value,
}

impl McpReadResourceResponse {
    /// The `contents` items (`uri`, `mimeType`, `text`, `blob` and `_meta`) when the response carries a list.
    pub fn contents(&self) -> Option<&[Value]> {
        self.response
            .get("contents")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
    }

    pub fn as_json(&self) -> &Value {
        &self.response
    }

    pub fn into_json(self) -> Value {
        self.response
    }
}

impl From<Value> for McpReadResourceResponse {
    fn from(response: Value) -> Self {
        Self { response }
    }
}
