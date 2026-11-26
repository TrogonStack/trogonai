use serde::{Deserialize, Serialize};

use crate::{LeaseId, TerminalId, TerminalOp};

/// A request to operate on one terminal of a lease.
///
/// The operation fields are flattened next to `lease_id` and `terminal_id`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TerminalRequest {
    pub lease_id: LeaseId,
    pub terminal_id: TerminalId,
    #[serde(flatten)]
    pub op: TerminalOp,
}
