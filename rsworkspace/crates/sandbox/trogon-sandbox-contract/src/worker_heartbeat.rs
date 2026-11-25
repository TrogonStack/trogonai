use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{
    CapabilitySupport, Digest, NativeVolumeBinding, ProviderCapabilityProfile, ProviderKind, WorkerCapacity,
    WorkerDispatchProtocol, WorkerId,
};

/// The periodic report a worker publishes so the broker can place leases on it.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkerHeartbeat {
    pub worker_id: WorkerId,
    #[serde(default)]
    pub dispatch_protocol: WorkerDispatchProtocol,
    /// Provider volumes the worker has bound to drive mounts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_volume_bindings: Vec<NativeVolumeBinding>,
    pub provider: ProviderKind,
    pub capacity: WorkerCapacity,
    pub emitted_at: SystemTime,
    /// Snapshot digests the worker already holds locally.
    #[serde(default)]
    pub cached_digests: Vec<Digest>,
    pub profile: ProviderCapabilityProfile,
    /// Idle warm sandboxes per snapshot digest, for providers that keep a warm pool.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub warm_pool_population: HashMap<Digest, u32>,
    /// Whether the worker understands lease epochs on dispatches and signed requests. The broker
    /// stamps an epoch only for a worker that supports it.
    #[serde(default)]
    pub supports_lease_epoch: CapabilitySupport,
}
