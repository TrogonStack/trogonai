use serde::{Deserialize, Serialize};

/// Why a lease ended.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseReleasedReason {
    #[default]
    Completed,
    Expired,
    Cancelled,
    /// No activity was seen for the lease's idle timeout.
    IdleTimeout,
    /// The provider lost the sandbox, because its VM died or the worker holding it restarted, so
    /// the lease ended without anyone asking.
    Failed,
}
