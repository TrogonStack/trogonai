use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures::stream::BoxStream;
use trogon_sandbox_contract::{LeaseId, TerminalId, TerminalOutput};

use super::session::Session;
use crate::lock::lock;

/// The sessions of every terminal this server opened, by lease and id.
#[derive(Default)]
pub(crate) struct Sessions {
    open: Mutex<HashMap<(LeaseId, TerminalId), Arc<Session>>>,
}

impl Sessions {
    /// Starts buffering `output` for the terminal, replacing any earlier session for the same id.
    pub(crate) fn attach(
        &self,
        lease_id: &LeaseId,
        terminal_id: &TerminalId,
        output: BoxStream<'static, TerminalOutput>,
    ) -> Arc<Session> {
        let session = Session::start(output);
        lock(&self.open).insert((lease_id.clone(), terminal_id.clone()), Arc::clone(&session));
        session
    }

    pub(crate) fn get(&self, lease_id: &LeaseId, terminal_id: &TerminalId) -> Option<Arc<Session>> {
        lock(&self.open).get(&(lease_id.clone(), terminal_id.clone())).cloned()
    }

    pub(crate) fn detach(&self, lease_id: &LeaseId, terminal_id: &TerminalId) {
        lock(&self.open).remove(&(lease_id.clone(), terminal_id.clone()));
    }
}

#[cfg(test)]
mod tests;
