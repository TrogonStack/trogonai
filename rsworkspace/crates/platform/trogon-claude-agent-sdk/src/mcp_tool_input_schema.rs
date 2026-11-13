use crate::{JsonObject, OutputSchema, OutputSchemaError};
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct McpToolInputSchema(OutputSchema);

#[derive(Debug, thiserror::Error)]
pub enum McpToolInputSchemaError {
    #[error("MCP tool input schemas must declare type object")]
    NotObject,
    #[error("invalid MCP tool input schema")]
    Schema(#[from] OutputSchemaError),
}

impl McpToolInputSchema {
    pub fn new(schema: JsonObject) -> Result<Self, McpToolInputSchemaError> {
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(McpToolInputSchemaError::NotObject);
        }
        Ok(Self(OutputSchema::new(schema)?))
    }

    pub fn as_json(&self) -> &JsonObject {
        self.0.as_json()
    }

    pub fn is_valid(&self, instance: &Value) -> bool {
        self.0.is_valid(instance)
    }

    pub(crate) fn validation_error(&self, instance: &Value) -> Option<String> {
        self.0.validation_error(instance)
    }
}
