use trogon_sandbox_contract::CredentialScope;
use trogon_sandbox_volumes::VolumePrefix;

/// How to write a read-write volume's changes back when its lease ends.
#[derive(Debug, Clone)]
pub(crate) struct VolumeSyncPlan {
    pub(crate) scope: CredentialScope,
    pub(crate) prefix: VolumePrefix,
}
