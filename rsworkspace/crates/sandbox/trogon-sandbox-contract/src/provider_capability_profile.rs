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
                SdkFeature::PauseResume => lifecycle.supports_pause_resume.is_supported(),
                SdkFeature::WriteStdin => isolation.supports_interactive_stdin.is_supported(),
                SdkFeature::RestartCommand => isolation.supports_command_restart.is_supported(),
                SdkFeature::CommandCancellation => isolation.supports_command_cancellation.is_supported(),
                SdkFeature::CommandLaunchCancellation => {
                    isolation.supports_command_cancellation.is_supported()
                        && isolation.supports_command_launch_cancellation.is_supported()
                }
                SdkFeature::Users => isolation.supports_sandbox_users.is_supported(),
                SdkFeature::Drives => !self.volume.supported_modes.is_empty(),
                SdkFeature::Snapshots => snapshot.can_snapshot.is_supported(),
                SdkFeature::Fork => {
                    snapshot.can_checkpoint_running.is_supported() && snapshot.can_restore.is_supported()
                }
                SdkFeature::UpdateNetworkPolicy => isolation.supports_network_policy_update.is_supported(),
                SdkFeature::SandboxDomain | SdkFeature::PortPreview => isolation.supports_port_preview.is_supported(),
                SdkFeature::ExposeAfterCreate => isolation.supports_expose_after_create.is_supported(),
                SdkFeature::EvidenceStreaming => isolation.supports_evidence_streaming.is_supported(),
                SdkFeature::Groups => isolation.supports_sandbox_groups.is_supported(),
                SdkFeature::ChangeTracking => isolation.supports_change_tracking.is_supported(),
                SdkFeature::FileWatch => isolation.supports_file_watch.is_supported(),
                SdkFeature::DirTransfer => isolation.supports_dir_transfer.is_supported(),
                SdkFeature::FileOps => isolation.supports_file_ops.is_supported(),
                SdkFeature::FileSearch => isolation.supports_file_search.is_supported(),
                SdkFeature::FileArchive => isolation.supports_file_archive.is_supported(),
                SdkFeature::Terminal => isolation.supports_terminal.is_supported(),
                SdkFeature::NamedProcesses => isolation.supports_named_processes.is_supported(),
                SdkFeature::GracefulStop => isolation.supports_graceful_stop.is_supported(),
                SdkFeature::PersistentRuntime => isolation.supports_persistent_runtime.is_supported(),
                SdkFeature::Browser => isolation.supports_browser.is_supported(),
                SdkFeature::OutputStreaming => isolation.supports_output_streaming.is_supported(),
                SdkFeature::UsageGauges => self.resources.reports_usage_gauges.is_supported(),
                SdkFeature::NativeAutoStop => lifecycle.supports_native_auto_stop.is_supported(),
                SdkFeature::NativePassthrough => self.placement.supports_native_passthrough.is_supported(),
            })
            .collect()
    }
}
