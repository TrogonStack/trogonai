use trogon_sandbox_contract::PullSecretId;
use trogon_sandbox_volumes::{VolumeEndpoint, VolumeRegion};

/// Where S3 workspace volumes live and which broker credential opens them.
#[derive(Debug, Clone)]
pub struct VolumeSettings {
    pub region: VolumeRegion,
    pub endpoint: Option<VolumeEndpoint>,
    pub credential_id: PullSecretId,
}
