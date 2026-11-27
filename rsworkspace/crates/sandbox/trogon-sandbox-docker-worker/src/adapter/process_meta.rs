use super::*;

/// A detached execution started with a [`ProcessSpec`].
#[derive(Clone)]
pub(super) struct ProcessMeta {
    pub(super) lease_id: LeaseId,
    pub(super) name: Option<ProcessName>,
    pub(super) readiness: Option<ReadinessProbe>,
    pub(super) container_id: String,
    pub(super) tap: OutputTap,
}
