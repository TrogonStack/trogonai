use trogon_sandbox_contract::{LeaseError, LeaseId, ProviderKind};

/// The provider and lease a [`LeaseError`] names, for callers that route or log by them.
pub trait LeaseErrorSubject {
    /// The provider this error names, when the variant carries one.
    fn provider(&self) -> Option<ProviderKind>;

    /// The lease this error names, when the variant carries one.
    fn lease_id(&self) -> Option<&LeaseId>;
}

impl LeaseErrorSubject for LeaseError {
    fn provider(&self) -> Option<ProviderKind> {
        match self {
            Self::SnapshotUnsupported { provider, .. }
            | Self::CheckpointRunningUnsupported { provider }
            | Self::ProviderLaunchFailed { provider, .. }
            | Self::InvalidNativeOptions { provider, .. }
            | Self::ProviderRejectedDispatch { provider, .. } => Some(*provider),
            _ => None,
        }
    }

    fn lease_id(&self) -> Option<&LeaseId> {
        match self {
            Self::SourceLeaseNotFound { lease_id }
            | Self::SourceLeaseNotRunning { lease_id }
            | Self::LeaseNotFound { lease_id }
            | Self::LeaseNotRunning { lease_id }
            | Self::SuspendUnsupported { lease_id, .. }
            | Self::LeaseWaking { lease_id, .. }
            | Self::SuspendArtifactLost { lease_id }
            | Self::SnapshotsDisabled { lease_id } => Some(lease_id),
            _ => None,
        }
    }
}
