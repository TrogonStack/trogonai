use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use trogon_sandbox_contract::CommandOutput;

/// The result of a detached command, kept so polling again returns the same answer.
#[derive(Debug)]
pub(crate) struct Finished {
    pub(crate) output: CommandOutput,
    pub(crate) at: Instant,
}

/// One detached command this worker started.
#[derive(Debug, Clone)]
pub(crate) struct Execution {
    pub(crate) lease_id: String,
    pub(crate) sandbox_id: String,
    pub(crate) dir: String,
    pub(crate) marker: String,
    pub(crate) max_output_bytes: usize,
    pub(crate) timeout_seconds: u32,
    pub(crate) finished: Arc<Mutex<Option<Finished>>>,
}

impl Execution {
    /// Whether the result has been kept for `ttl` already. A command still being read counts as not expired.
    pub(crate) fn expired(&self, ttl: Duration) -> bool {
        self.finished
            .try_lock()
            .is_ok_and(|finished| finished.as_ref().is_some_and(|done| done.at.elapsed() >= ttl))
    }
}
