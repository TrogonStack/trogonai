use trogon_sandbox_contract::{ExecutionId, LeaseId};

use crate::ulid::Ulid;

use super::constants::EXECUTION_DIRECTORY_PREFIX;
use super::deadline::Deadline;
use crate::snapshots::ProcessIdentity;

/// One detached command the worker holds custody of.
#[derive(Debug, Clone)]
pub(super) struct Execution {
    pub(super) lease_id: LeaseId,
    pub(super) dir: String,
    pub(super) user: String,
    pub(super) max_output_bytes: usize,
    pub(super) identity: ProcessIdentity,
    pub(super) deadline: Deadline,
}

impl Execution {
    /// A fresh, time-ordered identity for a new execution.
    pub(super) fn fresh_id() -> ExecutionId {
        ExecutionId::new(Ulid::generate().to_string())
    }

    pub(super) fn directory(execution_id: &ExecutionId) -> String {
        format!("{EXECUTION_DIRECTORY_PREFIX}{execution_id}")
    }
}
