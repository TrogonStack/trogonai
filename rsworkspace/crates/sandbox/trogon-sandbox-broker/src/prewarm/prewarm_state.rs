use super::*;

/// Per-digest prewarm policy and controller bookkeeping stored in NATS KV.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrewarmState {
    pub tenant: TenantId,
    pub digest: Digest,
    pub spec: PrewarmSpec,
    #[serde(default)]
    pub pending_dispatches: u32,
    #[serde(default)]
    pub popularity: u64,
    pub updated_at: SystemTime,
}

impl PrewarmState {
    pub fn new(tenant: TenantId, digest: Digest, spec: PrewarmSpec) -> Self {
        Self {
            tenant,
            digest,
            spec,
            pending_dispatches: 0,
            popularity: 0,
            updated_at: SystemTime::now(),
        }
    }
}
