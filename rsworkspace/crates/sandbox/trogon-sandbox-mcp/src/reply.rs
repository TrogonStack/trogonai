use rmcp::handler::server::wrapper::Json;
use serde::Serialize;

use crate::tool_error::ToolError;

/// What a tool returns when its output is free-form JSON.
pub(crate) type JsonReply = Result<Json<serde_json::Value>, ToolError>;

/// Serializes `value` as the tool's JSON output.
pub(crate) fn json_reply(value: &impl Serialize) -> JsonReply {
    serde_json::to_value(value).map(Json).map_err(ToolError::Encode)
}

#[cfg(test)]
mod tests;
