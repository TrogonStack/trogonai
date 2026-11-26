use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::ExecutionId;

/// Names a streamed execution before it prints anything.
///
/// The agent mints execution ids, and a command that prints nothing would otherwise never name itself, leaving the
/// worker unable to announce its exit to log followers.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ExecBegin {
    pub begin_execution_id: ExecutionId,
}
