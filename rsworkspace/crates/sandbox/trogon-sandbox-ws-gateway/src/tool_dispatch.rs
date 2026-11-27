use serde_json::Value;

use crate::{
    DispatchError, TerminalSessions, ToolContext, WsFrame, command_tools, domain_tools, drive_tools, entity_tools,
    file_tools, lease_tools, observability_tools, operation_tools, snapshot_tools, terminal_tools,
};

/// Answers request `id` with the outcome of its tool.
fn frame_of(id: String, result: Result<Value, DispatchError>) -> WsFrame {
    match result {
        Ok(result) => WsFrame::response_ok(id, result),
        Err(error) => error.into_frame(id),
    }
}

/// Runs one non-terminal tool and answers request `id`. Safe to run beside other requests.
pub(crate) async fn run_tool(ctx: &ToolContext, id: String, tool: &str, payload: &Value) -> WsFrame {
    frame_of(id, call_tool(ctx, tool, payload).await)
}

/// Runs one tool, terminals included, on the socket's own task and answers request `id`.
pub(crate) async fn run_inline(
    ctx: &ToolContext,
    terminals: &mut TerminalSessions,
    id: String,
    tool: &str,
    payload: &Value,
) -> WsFrame {
    let result = match terminal_tools::call(ctx, terminals, tool, payload).await {
        Ok(Some(result)) => Ok(result),
        Ok(None) => call_tool(ctx, tool, payload).await,
        Err(error) => Err(error),
    };
    frame_of(id, result)
}

/// Offers `tool` to each tool group in turn.
pub(crate) async fn call_tool(ctx: &ToolContext, tool: &str, payload: &Value) -> Result<Value, DispatchError> {
    if let Some(result) = operation_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = observability_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = lease_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = drive_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = command_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = file_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = domain_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = entity_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    if let Some(result) = snapshot_tools::call(ctx, tool, payload).await? {
        return Ok(result);
    }
    Err(DispatchError::UnknownTool(tool.to_string()))
}
