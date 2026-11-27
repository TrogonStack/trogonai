use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::SandboxUserName;

use super::captured_deadline::CapturedDeadline;
use super::process_identity::ProcessIdentity;

/// One command that was alive when a snapshot was captured.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapturedExecution {
    pub(crate) execution_id: String,
    pub(crate) user: SandboxUserName,
    pub(crate) identity: ProcessIdentity,
    pub(crate) max_output_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) deadline: Option<CapturedDeadline>,
}

impl CapturedExecution {
    /// The sandbox directory holding this command's control files.
    pub(crate) fn directory(&self) -> String {
        format!("/tmp/.trogon-exec-{}", self.execution_id)
    }
}
