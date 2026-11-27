use std::path::PathBuf;

use trogon_sandbox_contract::OciArtifactRef;

/// Where the guest of a new lease comes from: a Wasm component on disk, or a snapshot artifact
/// published to a registry.
#[derive(Debug)]
pub(crate) enum SandboxSource {
    Component(PathBuf),
    Snapshot(OciArtifactRef),
}
