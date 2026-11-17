use std::{future::Future, marker::PhantomData};

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    CallbackError, CancellationToken, JsonObject, McpToolArgumentsError, McpToolFuture, McpToolHandler, McpToolOutput,
    McpToolRequest, constants::TYPED_TOOL_ARGUMENTS_MAX_DEPTH, deep_json,
};

/// The arguments of a tool call read into `T`, with the call's cancellation signal.
#[derive(Clone, Debug)]
pub struct TypedMcpToolRequest<T> {
    pub arguments: T,
    pub signal: CancellationToken,
}

/// An [`McpToolHandler`] whose handler takes its arguments as `T`, like the typed handler of the TypeScript `tool()`
/// and the Python `SdkMcpTool[T]`.
///
/// The arguments are read into `T` after the server validates them against the tool's input schema. Arguments that
/// do not fit `T` never reach the handler: the call fails with [`McpToolArgumentsError`], which the server answers
/// as it answers arguments that break the input schema.
pub struct TypedMcpToolHandler<T, F> {
    handler: F,
    arguments: PhantomData<fn() -> T>,
}

impl<T, F, Fut> TypedMcpToolHandler<T, F>
where
    T: DeserializeOwned + Send + 'static,
    F: Fn(TypedMcpToolRequest<T>) -> Fut + Send + Sync,
    Fut: Future<Output = Result<McpToolOutput, CallbackError>> + Send + 'static,
{
    pub fn new(handler: F) -> Self {
        Self {
            handler,
            arguments: PhantomData,
        }
    }
}

impl<T, F, Fut> McpToolHandler for TypedMcpToolHandler<T, F>
where
    T: DeserializeOwned + Send + 'static,
    F: Fn(TypedMcpToolRequest<T>) -> Fut + Send + Sync,
    Fut: Future<Output = Result<McpToolOutput, CallbackError>> + Send + 'static,
{
    fn call(&self, request: McpToolRequest) -> McpToolFuture<'_> {
        let McpToolRequest { arguments, signal } = request;
        match read_arguments(arguments) {
            Ok(arguments) => Box::pin((self.handler)(TypedMcpToolRequest { arguments, signal })),
            Err(error) => Box::pin(std::future::ready(Err(CallbackError::Failed(Box::new(error))))),
        }
    }
}

/// `serde_json::from_value`, refusing arguments nested past [`TYPED_TOOL_ARGUMENTS_MAX_DEPTH`] since reading a value
/// recurses once per level.
fn read_arguments<T: DeserializeOwned>(arguments: JsonObject) -> Result<T, McpToolArgumentsError> {
    let arguments = Value::Object(arguments);
    if nests_past(&arguments, TYPED_TOOL_ARGUMENTS_MAX_DEPTH) {
        deep_json::drop_value(arguments);
        return Err(McpToolArgumentsError::new("recursion limit exceeded"));
    }
    serde_json::from_value(arguments).map_err(McpToolArgumentsError::from)
}

/// Whether `value` holds more than `limit` nested arrays and objects, walked without recursion.
fn nests_past(value: &Value, limit: usize) -> bool {
    let mut pending = vec![(value, 1_usize)];
    while let Some((value, depth)) = pending.pop() {
        let children: Vec<&Value> = match value {
            Value::Array(items) => items.iter().collect(),
            Value::Object(object) => object.values().collect(),
            _ => continue,
        };
        if depth > limit {
            return true;
        }
        pending.extend(children.into_iter().map(|child| (child, depth + 1)));
    }
    false
}

#[cfg(test)]
#[path = "typed_mcp_tool_handler_tests.rs"]
mod tests;
