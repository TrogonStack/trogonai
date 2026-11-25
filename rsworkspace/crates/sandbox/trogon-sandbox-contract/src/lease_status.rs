use serde::{Deserialize, Serialize};

/// Where a lease is in its lifecycle.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaseStatus {
    Pending,
    Running,
    /// Frozen by a pause request. State is kept and the lease still counts against the tenant's
    /// active lease cap.
    Paused,
    /// A suspend request is under way and the worker has been or is being asked to suspend the
    /// sandbox.
    Suspending,
    /// Host resources are released and a durable artifact is kept. The lease takes no verbs until
    /// it is woken.
    Suspended,
    /// A wake request is under way. Moving from `Suspended` to `Waking` claims the wake.
    Waking,
    Cancelling,
    Released,
    Failed,
}
