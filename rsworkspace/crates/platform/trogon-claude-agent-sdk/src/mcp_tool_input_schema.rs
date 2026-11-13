use crate::{JsonObject, OutputSchema, OutputSchemaError};
use serde_json::Value;

/// The input schema an SDK MCP tool advertises in `tools/list` and checks `tools/call` arguments
/// against.
///
/// Neither upstream SDK checks the declared schema when a tool is built, so `McpToolInputSchema::new`
/// keeps any object. `McpToolInputSchema::validated` is an opt-in check that also requires
/// `"type": "object"`, document-local references and a schema that compiles.
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
    pub fn new(schema: JsonObject) -> Self {
        Self(OutputSchema::new(schema))
    }

    /// Opt-in check that neither upstream SDK performs when a tool is built.
    pub fn validated(schema: JsonObject) -> Result<Self, McpToolInputSchemaError> {
        if schema.get("type").and_then(Value::as_str) != Some("object") {
            return Err(McpToolInputSchemaError::NotObject);
        }
        Ok(Self(OutputSchema::validated(schema)?))
    }

    pub fn as_json(&self) -> &JsonObject {
        self.0.as_json()
    }

    pub fn is_valid(&self, instance: &Value) -> bool {
        self.0.is_valid(instance)
    }

    /// The compile error of a schema that `McpToolInputSchema::new` accepted but could not compile.
    pub fn compile_error(&self) -> Option<&str> {
        self.0.compile_error()
    }

    pub(crate) fn validation_error(&self, instance: &Value) -> Option<String> {
        self.0.validation_error(instance)
    }
}

impl From<JsonObject> for McpToolInputSchema {
    fn from(schema: JsonObject) -> Self {
        Self::new(schema)
    }
}
