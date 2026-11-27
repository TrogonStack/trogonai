use super::VerbScope;

/// Every client-facing request subject the broker answers. Adding a variant
/// forces [`ClientVerb::scope`] to classify it, so a new verb cannot ship
/// without deciding how it is authenticated.
///
/// Deliberately absent: the capability list (no tenant or lease on the
/// request, provider profiles are not tenant data) and the worker-published
/// subjects (heartbeats, activity, snapshot events, evidence).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientVerb {
    LeaseRequest,
    LeaseFork,
    Cancel,
    TagsUpdate,
    NetworkPolicyUpdate,
    ExtendTimeout,
    LeaseUsage,
    TenantUsage,
    TenantLimits,
    DeleteLease,
    StopLease,
    PauseLease,
    ResumeLease,
    SuspendLease,
    WakeLease,
    RollbackLease,
    SnapshotLease,
    SnapshotDelete,
    NativeSnapshotDelete,
    RunCommand,
    PollCommand,
    KillCommand,
    CancelCommandLaunch,
    StopCommand,
    RestartCommand,
    ReadLogs,
    WriteStdin,
    ListProcesses,
    WaitReady,
    WaitSandboxReady,
    GetDomain,
    Terminal,
    NativeHandle,
    SshAccess,
    WriteFiles,
    ReadFile,
    FileOp,
    CreateUser,
    CreateGroup,
    ModifyGroupMembership,
    DriveCreate,
    DriveList,
    DriveDelete,
    LeaseList,
    LeaseReap,
    GetLeaseByName,
    SnapshotGet,
    SnapshotTree,
    SnapshotTagSet,
    SnapshotTagDelete,
    SnapshotTagHistory,
    DeleteNamed,
    OperationGet,
    OperationCancel,
    OperationCapabilities,
    GetEntity,
    ListEntities,
    DeleteEntity,
}

impl ClientVerb {
    #[cfg(test)]
    pub const ALL: &'static [ClientVerb] = &[
        Self::LeaseRequest,
        Self::LeaseFork,
        Self::Cancel,
        Self::TagsUpdate,
        Self::NetworkPolicyUpdate,
        Self::ExtendTimeout,
        Self::LeaseUsage,
        Self::TenantUsage,
        Self::TenantLimits,
        Self::DeleteLease,
        Self::StopLease,
        Self::PauseLease,
        Self::ResumeLease,
        Self::SuspendLease,
        Self::WakeLease,
        Self::RollbackLease,
        Self::SnapshotLease,
        Self::SnapshotDelete,
        Self::NativeSnapshotDelete,
        Self::RunCommand,
        Self::PollCommand,
        Self::KillCommand,
        Self::CancelCommandLaunch,
        Self::StopCommand,
        Self::RestartCommand,
        Self::ReadLogs,
        Self::WriteStdin,
        Self::ListProcesses,
        Self::WaitReady,
        Self::WaitSandboxReady,
        Self::GetDomain,
        Self::Terminal,
        Self::NativeHandle,
        Self::SshAccess,
        Self::WriteFiles,
        Self::ReadFile,
        Self::FileOp,
        Self::CreateUser,
        Self::CreateGroup,
        Self::ModifyGroupMembership,
        Self::DriveCreate,
        Self::DriveList,
        Self::DriveDelete,
        Self::LeaseList,
        Self::LeaseReap,
        Self::GetLeaseByName,
        Self::SnapshotGet,
        Self::SnapshotTree,
        Self::SnapshotTagSet,
        Self::SnapshotTagDelete,
        Self::SnapshotTagHistory,
        Self::DeleteNamed,
        Self::OperationGet,
        Self::OperationCancel,
        Self::OperationCapabilities,
        Self::GetEntity,
        Self::ListEntities,
        Self::DeleteEntity,
    ];

    pub const fn scope(self) -> VerbScope {
        match self {
            Self::Cancel
            | Self::TagsUpdate
            | Self::NetworkPolicyUpdate
            | Self::ExtendTimeout
            | Self::RunCommand
            | Self::PollCommand
            | Self::KillCommand
            | Self::CancelCommandLaunch
            | Self::StopCommand
            | Self::RestartCommand
            | Self::ReadLogs
            | Self::WriteStdin
            | Self::ListProcesses
            | Self::WaitReady
            | Self::WaitSandboxReady
            | Self::GetDomain
            | Self::Terminal
            | Self::NativeHandle
            | Self::SshAccess
            | Self::WriteFiles
            | Self::ReadFile
            | Self::FileOp
            | Self::CreateUser
            | Self::CreateGroup
            | Self::ModifyGroupMembership => VerbScope::Lease,
            // Requests that carry both `tenant` and `lease_id` are bound to
            // the request tenant: the handler rejects a lease owned by
            // another tenant, so that is sufficient.
            Self::LeaseRequest
            | Self::LeaseFork
            | Self::LeaseUsage
            | Self::TenantUsage
            | Self::TenantLimits
            | Self::DeleteLease
            | Self::StopLease
            | Self::PauseLease
            | Self::ResumeLease
            | Self::SuspendLease
            | Self::WakeLease
            | Self::RollbackLease
            | Self::SnapshotLease
            | Self::SnapshotDelete
            | Self::NativeSnapshotDelete
            | Self::DriveCreate
            | Self::DriveList
            | Self::DriveDelete
            | Self::LeaseList
            | Self::LeaseReap
            | Self::GetLeaseByName
            | Self::SnapshotGet
            | Self::SnapshotTree
            | Self::SnapshotTagSet
            | Self::SnapshotTagDelete
            | Self::SnapshotTagHistory
            | Self::DeleteNamed
            | Self::OperationGet
            | Self::OperationCancel
            | Self::OperationCapabilities
            | Self::GetEntity
            | Self::ListEntities
            | Self::DeleteEntity => VerbScope::Tenant,
        }
    }
}
