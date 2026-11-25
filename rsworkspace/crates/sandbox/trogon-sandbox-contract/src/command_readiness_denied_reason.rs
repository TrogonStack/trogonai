use serde::{Deserialize, Serialize};

/// Why a worker refused a command readiness request.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandReadinessDeniedReason {
    InvalidBinding,
    ForeignBinding,
    UnknownBinding,
    InvalidRequest,
}
