//! `sandbox_terminal_*` tools: interactive PTY terminals over the socket.
//!
//! The gateway, not the client, picks the terminal id, and only after it has subscribed to that id's output. A client
//! therefore cannot name another tenant's terminal to listen in on it, and never misses the first prompt. Input,
//! resize and close are accepted only for terminals this socket opened; the broker authenticates every frame again
//! against the lease's tenant. Terminals still open when the socket drops are closed.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    LeaseId, TerminalArgument, TerminalDimension, TerminalError, TerminalId, TerminalInput, TerminalOp,
    TerminalResponse,
};

use crate::constants::{MAX_TERMINALS_PER_SOCKET, TERMINAL_CLEANUP_TIMEOUT, TERMINAL_ID_BYTES};
use crate::encode::encode;
use crate::from_payload::from_payload;
use crate::random_hex::random_hex;
use crate::response_bodies::OutcomeBody;
use crate::terminal_sessions::OpenTerminal;
use crate::{DispatchError, TerminalSessions, ToolContext};

#[derive(Deserialize)]
struct OpenPayload {
    lease_id: LeaseId,
    cols: TerminalDimension,
    rows: TerminalDimension,
    #[serde(default)]
    cmd: Vec<TerminalArgument>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct InputPayload {
    terminal_id: TerminalId,
    data: TerminalInput,
}

#[derive(Deserialize)]
struct ResizePayload {
    terminal_id: TerminalId,
    cols: TerminalDimension,
    rows: TerminalDimension,
}

#[derive(Deserialize)]
struct ClosePayload {
    terminal_id: TerminalId,
}

#[derive(Debug, serde::Serialize)]
struct OpenedBody {
    outcome: &'static str,
    terminal_id: TerminalId,
}

/// Runs a terminal tool. `Ok(None)` when `tool` is not one.
pub(crate) async fn call(
    ctx: &ToolContext,
    terminals: &mut TerminalSessions,
    tool: &str,
    payload: &Value,
) -> Result<Option<Value>, DispatchError> {
    let value = match tool {
        "sandbox_terminal_open" => open(ctx, terminals, from_payload(payload)?).await?,
        "sandbox_terminal_input" => {
            let input: InputPayload = from_payload(payload)?;
            forward(
                ctx,
                terminals,
                &input.terminal_id,
                TerminalOp::Input { data: input.data },
            )
            .await?
        }
        "sandbox_terminal_resize" => {
            let resize: ResizePayload = from_payload(payload)?;
            let op = TerminalOp::Resize {
                cols: resize.cols,
                rows: resize.rows,
            };
            forward(ctx, terminals, &resize.terminal_id, op).await?
        }
        "sandbox_terminal_close" => close(ctx, terminals, from_payload(payload)?).await?,
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// Turns a broker reply for a non-open operation into the result.
fn reply(result: Result<TerminalResponse, ClientError>) -> Result<Value, DispatchError> {
    match result {
        Ok(TerminalResponse::Done | TerminalResponse::Opened { .. }) => encode(OutcomeBody { outcome: "done" }),
        Ok(TerminalResponse::Failed(error)) => Err(error.into()),
        Ok(TerminalResponse::Unsupported) => Err(DispatchError::TerminalUnsupported),
        Ok(TerminalResponse::Denied(error)) => Err(ClientError::Denied(error).into()),
        Err(error) => Err(error.into()),
    }
}

async fn open(ctx: &ToolContext, terminals: &mut TerminalSessions, open: OpenPayload) -> Result<Value, DispatchError> {
    if terminals.len() >= MAX_TERMINALS_PER_SOCKET {
        return Err(TerminalError::TooManyTerminals {
            max: MAX_TERMINALS_PER_SOCKET as u32,
        }
        .into());
    }
    let terminal_id = TerminalId::new(format!("t{}", random_hex(TERMINAL_ID_BYTES)?))?;
    let op = TerminalOp::Open {
        cols: open.cols,
        rows: open.rows,
        cmd: open.cmd,
        env: open.env,
        cwd: open.cwd,
        resume_from: None,
    };
    let stop = CancellationToken::new();
    ctx.backend
        .subscribe_terminal(terminal_id.clone(), ctx.events.clone(), stop.clone())
        .await?;
    let result = ctx
        .backend
        .terminal(ctx.api_key(), &open.lease_id, terminal_id.clone(), op)
        .await;
    if let Ok(TerminalResponse::Opened { .. }) = result {
        terminals.insert(
            terminal_id.clone(),
            OpenTerminal {
                lease_id: open.lease_id,
                stop,
            },
        );
        return encode(OpenedBody {
            outcome: "opened",
            terminal_id,
        });
    }
    stop.cancel();
    // A timeout or lost connection may still have opened it: do not leak a PTY.
    if matches!(result, Err(ClientError::Timeout(_) | ClientError::Transport(_))) {
        let close = ctx
            .backend
            .terminal(ctx.api_key(), &open.lease_id, terminal_id, TerminalOp::Close);
        let _ = tokio::time::timeout(TERMINAL_CLEANUP_TIMEOUT, close).await;
    }
    reply(result)
}

async fn forward(
    ctx: &ToolContext,
    terminals: &TerminalSessions,
    terminal_id: &TerminalId,
    op: TerminalOp,
) -> Result<Value, DispatchError> {
    let Some(open) = terminals.get(terminal_id) else {
        return Err(TerminalError::NotFound.into());
    };
    reply(
        ctx.backend
            .terminal(ctx.api_key(), &open.lease_id, terminal_id.clone(), op)
            .await,
    )
}

async fn close(
    ctx: &ToolContext,
    terminals: &mut TerminalSessions,
    close: ClosePayload,
) -> Result<Value, DispatchError> {
    let Some(open) = terminals.remove(&close.terminal_id) else {
        // Closing an id this socket does not hold is a no-op, so a retried close is safe.
        return encode(OutcomeBody { outcome: "done" });
    };
    open.stop.cancel();
    reply(
        ctx.backend
            .terminal(ctx.api_key(), &open.lease_id, close.terminal_id, TerminalOp::Close)
            .await,
    )
}
