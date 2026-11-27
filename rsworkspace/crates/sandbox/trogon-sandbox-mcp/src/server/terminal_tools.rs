use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{tool, tool_router};
use trogon_sandbox_contract::{LeaseId, TerminalArgument, TerminalDimension, TerminalId, TerminalInput, TerminalOp};

use super::SandboxMcpServer;
use super::terminal_outcome::terminal_outcome;
use crate::ack::Ack;
use crate::constants::{DEFAULT_READ_BYTES, DEFAULT_TERMINAL_COLS, DEFAULT_TERMINAL_ROWS, MAX_READ_WAIT_MS};
use crate::inputs::terminal_inputs::{
    TerminalCloseInput, TerminalOpenInput, TerminalOpenOutput, TerminalReadInput, TerminalReadOutput,
    TerminalResizeInput, TerminalWriteInput,
};
use crate::parse;
use crate::terminal::fresh_terminal_id;
use crate::tool_error::ToolError;

impl SandboxMcpServer {
    async fn terminal_op(&self, lease_id: &LeaseId, terminal_id: &TerminalId, op: TerminalOp) -> Result<(), ToolError> {
        let response = self.client()?.terminal(lease_id, terminal_id.clone(), op).await?;
        terminal_outcome(response)
    }
}

fn dimension(field: &'static str, value: u16) -> Result<TerminalDimension, ToolError> {
    TerminalDimension::new(value).map_err(|e| ToolError::invalid(field, e))
}

fn terminal_id(value: &str) -> Result<TerminalId, ToolError> {
    TerminalId::new(value).map_err(|e| ToolError::invalid("terminal_id", e))
}

/// What to type, as the base64 the broker takes. Exactly one of the two sources must be set.
fn typed_input(text: Option<String>, data_base64: Option<String>) -> Result<TerminalInput, ToolError> {
    let encoded = match (text, data_base64) {
        (Some(text), None) => STANDARD.encode(text),
        (None, Some(data)) => data,
        _ => {
            return Err(ToolError::InvalidCombination {
                option: "text and data_base64",
                requires: "exactly one of them",
            });
        }
    };
    TerminalInput::new(encoded).map_err(|e| ToolError::invalid("data_base64", e))
}

#[tool_router(router = terminal_router, vis = "pub(super)")]
impl SandboxMcpServer {
    #[tool(
        description = "Open an interactive terminal (a PTY) in a running lease's sandbox and start buffering its output. Read the output with sandbox_terminal_read, type with sandbox_terminal_write, and end it with sandbox_terminal_close. Refused when the server has a command policy, since typed input cannot be checked against it."
    )]
    async fn sandbox_terminal_open(
        &self,
        params: Parameters<TerminalOpenInput>,
    ) -> Result<Json<TerminalOpenOutput>, ToolError> {
        self.state.require_auth()?;
        if !self.state.policy().commands.is_empty() {
            return Err(ToolError::TerminalUnderCommandPolicy);
        }
        let p = params.0;
        let lease_id = parse::lease_id(p.lease_id);
        let id = match p.terminal_id {
            Some(value) => terminal_id(&value)?,
            None => fresh_terminal_id().map_err(ToolError::TerminalId)?,
        };
        let cols = dimension("cols", p.cols.unwrap_or(DEFAULT_TERMINAL_COLS))?;
        let rows = dimension("rows", p.rows.unwrap_or(DEFAULT_TERMINAL_ROWS))?;
        let cmd = p
            .command
            .into_iter()
            .map(|argument| TerminalArgument::new(argument).map_err(|e| ToolError::invalid("command", e)))
            .collect::<Result<Vec<_>, _>>()?;
        let client = self.client()?;
        // Subscribed before the open so the first prompt is not lost.
        let output = client.terminal_output(&id).await?;
        self.terminals.attach(&lease_id, &id, output);
        let op = TerminalOp::Open {
            cols,
            rows,
            cmd,
            env: p.env,
            cwd: p.cwd,
            resume_from: None,
        };
        if let Err(e) = self.terminal_op(&lease_id, &id, op).await {
            self.terminals.detach(&lease_id, &id);
            return Err(e);
        }
        Ok(Json(TerminalOpenOutput {
            terminal_id: id.to_string(),
        }))
    }

    #[tool(
        description = "Type into a terminal opened with sandbox_terminal_open. Send text (\\r is Enter, \\u0003 is Ctrl-C) or raw bytes as base64."
    )]
    async fn sandbox_terminal_write(&self, params: Parameters<TerminalWriteInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let p = params.0;
        let data = typed_input(p.text, p.data_base64)?;
        let id = terminal_id(&p.terminal_id)?;
        self.terminal_op(&parse::lease_id(p.lease_id), &id, TerminalOp::Input { data })
            .await?;
        Ok(Ack::reply())
    }

    #[tool(
        description = "Read what a terminal printed since the last read, waiting up to wait_ms for output when none is buffered. exited turns true once the terminal ended and its output is all read."
    )]
    async fn sandbox_terminal_read(
        &self,
        params: Parameters<TerminalReadInput>,
    ) -> Result<Json<TerminalReadOutput>, ToolError> {
        self.state.require_auth()?;
        let p = params.0;
        let lease_id = parse::lease_id(p.lease_id);
        let id = terminal_id(&p.terminal_id)?;
        let session = self
            .terminals
            .get(&lease_id, &id)
            .ok_or_else(|| ToolError::TerminalNotOpen(p.terminal_id.clone()))?;
        let wait = Duration::from_millis(u64::from(p.wait_ms.unwrap_or(0).min(MAX_READ_WAIT_MS)));
        let max_bytes = p.max_bytes.map_or(DEFAULT_READ_BYTES, |n| n as usize);
        let read = session.read(max_bytes, wait).await;
        Ok(Json(TerminalReadOutput {
            output: read.output,
            dropped_bytes: read.dropped_bytes,
            missed_frames: read.missed_frames,
            exited: read.exited,
            exit_code: read.exit_code,
        }))
    }

    #[tool(description = "Change the size of a terminal opened with sandbox_terminal_open.")]
    async fn sandbox_terminal_resize(&self, params: Parameters<TerminalResizeInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let p = params.0;
        let op = TerminalOp::Resize {
            cols: dimension("cols", p.cols)?,
            rows: dimension("rows", p.rows)?,
        };
        let id = terminal_id(&p.terminal_id)?;
        self.terminal_op(&parse::lease_id(p.lease_id), &id, op).await?;
        Ok(Ack::reply())
    }

    #[tool(description = "End a terminal and drop its unread output. Closing one that already ended is harmless.")]
    async fn sandbox_terminal_close(&self, params: Parameters<TerminalCloseInput>) -> Result<Json<Ack>, ToolError> {
        self.state.require_auth()?;
        let p = params.0;
        let lease_id = parse::lease_id(p.lease_id);
        let id = terminal_id(&p.terminal_id)?;
        let closed = self.terminal_op(&lease_id, &id, TerminalOp::Close).await;
        self.terminals.detach(&lease_id, &id);
        closed?;
        Ok(Ack::reply())
    }
}
