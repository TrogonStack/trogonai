use trogon_sandbox_contract::{ProcessName, ReadinessProbe};

use crate::LiveOutput;

/// A detached execution started with a process spec.
pub(crate) struct Proc {
    pub name: Option<ProcessName>,
    pub readiness: Option<ReadinessProbe>,
    pub live: LiveOutput,
}
