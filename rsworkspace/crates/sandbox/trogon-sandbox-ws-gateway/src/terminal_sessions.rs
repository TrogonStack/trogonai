use std::collections::HashMap;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{LeaseId, TerminalId, TerminalOp};

use crate::ToolContext;
use crate::constants::TERMINAL_CLEANUP_TIMEOUT;

/// A terminal this socket opened.
pub(crate) struct OpenTerminal {
    pub lease_id: LeaseId,
    pub stop: CancellationToken,
}

/// The terminals one socket opened. Input, resize and close are accepted only for these.
#[derive(Default)]
pub struct TerminalSessions {
    open: HashMap<TerminalId, OpenTerminal>,
}

impl TerminalSessions {
    pub(crate) fn len(&self) -> usize {
        self.open.len()
    }

    pub(crate) fn insert(&mut self, terminal_id: TerminalId, terminal: OpenTerminal) {
        self.open.insert(terminal_id, terminal);
    }

    pub(crate) fn get(&self, terminal_id: &TerminalId) -> Option<&OpenTerminal> {
        self.open.get(terminal_id)
    }

    pub(crate) fn remove(&mut self, terminal_id: &TerminalId) -> Option<OpenTerminal> {
        self.open.remove(terminal_id)
    }

    /// Closes every terminal this socket still holds. Best effort and bounded: the socket is already gone, and the
    /// worker also ends terminals when the lease is released.
    pub(crate) async fn close_all(&mut self, ctx: &ToolContext) {
        for (terminal_id, open) in self.open.drain() {
            open.stop.cancel();
            let close = ctx
                .backend
                .terminal(ctx.api_key(), &open.lease_id, terminal_id, TerminalOp::Close);
            let _ = tokio::time::timeout(TERMINAL_CLEANUP_TIMEOUT, close).await;
        }
    }
}
