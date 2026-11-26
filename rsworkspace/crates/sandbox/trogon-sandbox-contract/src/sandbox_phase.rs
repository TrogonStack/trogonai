use serde::{Deserialize, Serialize};

/// A normalized sandbox phase for timelines and telemetry.
///
/// A provider reports only the phases it actually performs.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPhase {
    Allocate,
    RestoreSnapshot,
    MountFilesystem,
    PrepareWorkspace,
    PullImage,
    PullingImage,
    BootRuntime,
    Execute,
    Snapshotting,
    Reclaim,
}
