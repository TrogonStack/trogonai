use trogon_sandbox_contract::LeaseId;

/// Something that happened to an entity.
///
/// Events that carry a lease must name the lease the entity is bound to, so a stale writer cannot move a newer run.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EntityEvent {
    /// Creation finished and the entity can start.
    Provisioned,
    /// A start was claimed with this lease.
    StartRequested { lease: LeaseId },
    /// The lease is up.
    LeaseReady { lease: LeaseId },
    /// The run was paused.
    Paused { lease: LeaseId },
    /// The run was resumed.
    Resumed { lease: LeaseId },
    /// A stop was requested for the bound lease.
    StopRequested { lease: LeaseId },
    /// The lease ended: released, expired, lost, or a start abandoned.
    RunEnded { lease: LeaseId },
    /// A delete was requested. Legal from any live state. A bound lease stays recorded as the current lease so the
    /// delete can still end the run.
    DeleteRequested,
    /// The entity failed.
    Failed { reason: String },
    /// An operator or sweep cleared a failure.
    Recovered,
}

impl EntityEvent {
    /// The stable snake_case name of the event, for logs and errors.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Provisioned => "provisioned",
            Self::StartRequested { .. } => "start_requested",
            Self::LeaseReady { .. } => "lease_ready",
            Self::Paused { .. } => "paused",
            Self::Resumed { .. } => "resumed",
            Self::StopRequested { .. } => "stop_requested",
            Self::RunEnded { .. } => "run_ended",
            Self::DeleteRequested => "delete_requested",
            Self::Failed { .. } => "failed",
            Self::Recovered => "recovered",
        }
    }
}
