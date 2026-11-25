use serde::{Deserialize, Serialize};

use crate::{ExecutionId, LeaseId, UnixSignal};

/// A request to send a Unix signal to a previously started detached command.
///
/// Without a `signal` it sends `TERM`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct KillCommandRequest {
    pub lease_id: LeaseId,
    pub execution_id: ExecutionId,
    #[serde(default)]
    pub signal: UnixSignal,
}
