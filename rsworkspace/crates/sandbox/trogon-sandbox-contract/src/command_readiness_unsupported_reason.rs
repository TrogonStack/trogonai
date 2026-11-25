use serde::{Deserialize, Serialize};

/// Why a sandbox can never run commands through the sealed supervisor.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandReadinessUnsupportedReason {
    IncompatibleIsolation,
    RuntimeNotConfigured,
    LegacyAssignment,
}
