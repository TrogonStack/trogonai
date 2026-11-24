use serde::{Deserialize, Serialize};

/// Why a sandbox that supports the sealed supervisor cannot run commands through it right now.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandReadinessUnavailableReason {
    RuntimeUnavailable,
    ProbeFailed,
    CustodyUnavailable,
    AuthorityUnavailable,
}
