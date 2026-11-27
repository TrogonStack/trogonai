/// A workspace volume snapshot poll interval or timeout was zero.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("workspace volume snapshot timings must be non-zero")]
pub struct FlyWorkspaceVolumeError;
