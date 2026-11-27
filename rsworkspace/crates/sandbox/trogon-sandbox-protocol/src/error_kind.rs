use trogon_sandbox_contract::{DriveError, LeaseError, TerminalError};

/// The stable snake_case `kind` tag an error carries on the wire.
pub trait ErrorKind {
    /// The tag this variant serializes under.
    fn kind(&self) -> &'static str;
}

impl ErrorKind for LeaseError {
    fn kind(&self) -> &'static str {
        match self {
            Self::QuotaExceeded { .. } => "quota_exceeded",
            Self::NoProviderAvailable => "no_provider_available",
            Self::InvalidSpec { .. } => "invalid_spec",
            Self::InvalidNetworkPolicy { .. } => "invalid_network_policy",
            Self::Backend { .. } => "backend",
            Self::Unauthorized { .. } => "unauthorized",
            Self::NoWorkerSatisfiesCapabilities { .. } => "no_worker_satisfies_capabilities",
            Self::NoWorkerCanRestoreSnapshot { .. } => "no_worker_can_restore_snapshot",
            Self::TooManyActiveLeases { .. } => "too_many_active_leases",
            Self::TooManyTaggedLeases { .. } => "too_many_tagged_leases",
            Self::SnapshotQuotaExceeded { .. } => "snapshot_quota_exceeded",
            Self::SnapshotTagNotFound { .. } => "snapshot_tag_not_found",
            Self::SnapshotUnsupported { .. } => "snapshot_unsupported",
            Self::SnapshotSignatureInvalid { .. } => "snapshot_signature_invalid",
            Self::PrewarmQuotaExceeded { .. } => "prewarm_quota_exceeded",
            Self::ImageNotReady { .. } => "image_not_ready",
            Self::SourceLeaseNotFound { .. } => "source_lease_not_found",
            Self::SourceLeaseNotRunning { .. } => "source_lease_not_running",
            Self::CheckpointRunningUnsupported { .. } => "checkpoint_running_unsupported",
            Self::SnapshotsDisabled { .. } => "snapshots_disabled",
            Self::CheckpointRunningFailed { .. } => "checkpoint_running_failed",
            Self::LeaseNotFound { .. } => "lease_not_found",
            Self::LeaseNotRunning { .. } => "lease_not_running",
            Self::InvalidTags { .. } => "invalid_tags",
            Self::WouldExceedMaxLifetime { .. } => "would_exceed_max_lifetime",
            Self::ProviderLaunchFailed { .. } => "provider_launch_failed",
            Self::ProviderRejectedDispatch { .. } => "provider_rejected_dispatch",
            Self::FileTooLargeForTransport { .. } => "file_too_large_for_transport",
            Self::CommandOutputTooLargeForTransport { .. } => "command_output_too_large_for_transport",
            Self::FilesTooLargeForTransport { .. } => "files_too_large_for_transport",
            Self::NameConflict { .. } => "name_conflict",
            Self::NativePassthroughNotPermitted { .. } => "native_passthrough_not_permitted",
            Self::InvalidNativeOptions { .. } => "invalid_native_options",
            Self::NoWorkerInRegion { .. } => "no_worker_in_region",
            Self::OperationConflict { .. } => "operation_conflict",
            Self::OperationNotFound { .. } => "operation_not_found",
            Self::OperationCanceled { .. } => "operation_canceled",
            Self::SuspendUnsupported { .. } => "suspend_unsupported",
            Self::LeaseWaking { .. } => "lease_waking",
            Self::SuspendArtifactLost { .. } => "suspend_artifact_lost",
            Self::EntityUnavailable { .. } => "entity_unavailable",
            Self::EntityNotEnabled => "entity_not_enabled",
        }
    }
}

impl ErrorKind for DriveError {
    fn kind(&self) -> &'static str {
        match self {
            Self::DriveNotFound { .. } => "drive_not_found",
            Self::DriveAlreadyExists { .. } => "drive_already_exists",
            Self::DriveAttached { .. } => "drive_attached",
            Self::DriveSizeExceeded { .. } => "drive_size_exceeded",
            Self::TooManyDrives { .. } => "too_many_drives",
            Self::DriveMountPathOverlap { .. } => "drive_mount_path_overlap",
            Self::InvalidDriveMountPath { .. } => "invalid_drive_mount_path",
            Self::StorageBackend { .. } => "storage_backend",
            Self::Unauthorized { .. } => "unauthorized",
        }
    }
}

impl ErrorKind for TerminalError {
    fn kind(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::IdInUse => "id_in_use",
            Self::TooManyTerminals { .. } => "too_many_terminals",
            Self::InvalidRequest { .. } => "invalid_request",
            Self::SpawnFailed { .. } => "spawn_failed",
            Self::Exited => "exited",
        }
    }
}
