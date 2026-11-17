/// Tool arguments that passed the input schema but do not fit the type a handler reads them into.
///
/// A handler that fails with this error, as [`TypedMcpToolHandler`](crate::TypedMcpToolHandler) does, is answered
/// with the dialect's invalid arguments result: `MCP error -32602: Input validation error: Invalid arguments for tool
/// <name>: <message>` for the TypeScript dialect and `Input validation error: <message>` for the Python dialect,
/// both as an `isError` tool result.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{message}")]
pub struct McpToolArgumentsError {
    message: String,
}

impl McpToolArgumentsError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl From<serde_json::Error> for McpToolArgumentsError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}
