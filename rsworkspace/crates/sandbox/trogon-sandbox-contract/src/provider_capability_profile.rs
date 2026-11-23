use serde::{Deserialize, Serialize};

use crate::{
    IngressCapability, IsolationCapability, LifecycleCapability, PerformanceProfile, PlacementCapability, ProviderKind,
    ResourceCapability, SdkFeature, SnapshotCapability, VolumeCapability,
};

/// Everything one provider's worker declares it can do.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderCapabilityProfile {
    pub provider: ProviderKind,
    pub snapshot: SnapshotCapability,
    pub volume: VolumeCapability,
    pub isolation: IsolationCapability,
    pub lifecycle: LifecycleCapability,
    pub performance: PerformanceProfile,
    /// Absent in older profiles, which means no sizing.
    #[serde(default)]
    pub resources: ResourceCapability,
    #[serde(default)]
    pub placement: PlacementCapability,
    /// Absent in older profiles, which means unlimited.
    #[serde(default)]
    pub ingress: IngressCapability,
}

impl ProviderCapabilityProfile {
    /// The client-visible features this profile vouches for, in [`SdkFeature::ALL`] order.
    ///
    /// A feature without a backing profile flag is left out rather than guessed.
    pub fn sdk_features(&self) -> Vec<SdkFeature> {
        let isolation = &self.isolation;
        let lifecycle = &self.lifecycle;
        let snapshot = &self.snapshot;
        SdkFeature::ALL
            .into_iter()
            .filter(|feature| match feature {
                SdkFeature::PauseResume => lifecycle.supports_pause_resume,
                SdkFeature::WriteStdin => isolation.supports_interactive_stdin,
                SdkFeature::RestartCommand => isolation.supports_command_restart,
                SdkFeature::CommandCancellation => isolation.supports_command_cancellation,
                SdkFeature::CommandLaunchCancellation => {
                    isolation.supports_command_cancellation && isolation.supports_command_launch_cancellation
                }
                SdkFeature::Users => isolation.supports_sandbox_users,
                SdkFeature::Drives => !self.volume.supported_modes.is_empty(),
                SdkFeature::Snapshots => snapshot.can_snapshot,
                SdkFeature::Fork => snapshot.can_checkpoint_running && snapshot.can_restore,
                SdkFeature::UpdateNetworkPolicy => isolation.supports_network_policy_update,
                SdkFeature::SandboxDomain | SdkFeature::PortPreview => isolation.supports_port_preview,
                SdkFeature::ExposeAfterCreate => isolation.supports_expose_after_create,
                SdkFeature::EvidenceStreaming => isolation.supports_evidence_streaming,
                SdkFeature::Groups => isolation.supports_sandbox_groups,
                SdkFeature::ChangeTracking => isolation.supports_change_tracking,
                SdkFeature::FileWatch => isolation.supports_file_watch,
                SdkFeature::DirTransfer => isolation.supports_dir_transfer,
                SdkFeature::FileOps => isolation.supports_file_ops,
                SdkFeature::FileSearch => isolation.supports_file_search,
                SdkFeature::FileArchive => isolation.supports_file_archive,
                SdkFeature::Terminal => isolation.supports_terminal,
                SdkFeature::NamedProcesses => isolation.supports_named_processes,
                SdkFeature::GracefulStop => isolation.supports_graceful_stop,
                SdkFeature::PersistentRuntime => isolation.supports_persistent_runtime,
                SdkFeature::Browser => isolation.supports_browser,
                SdkFeature::OutputStreaming => isolation.supports_output_streaming,
                SdkFeature::UsageGauges => self.resources.reports_usage_gauges,
                SdkFeature::NativeAutoStop => lifecycle.supports_native_auto_stop,
                SdkFeature::NativePassthrough => self.placement.supports_native_passthrough,
            })
            .collect()
    }
}
