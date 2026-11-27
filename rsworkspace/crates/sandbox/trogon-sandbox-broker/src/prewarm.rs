use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use async_nats::jetstream;
use async_nats::jetstream::kv;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use trogon_sandbox_contract::{
    ArtifactType, Digest, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, LeaseRequestKeyError,
    LeaseResponse, OciArtifactRef, PrewarmSpec, ProviderKind, SandboxImage, SandboxSpec, SandboxSpecError,
    SandboxSpecParts, SandboxTags, TenantId, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::{PREWARM_KV_BUCKET, legacy_prewarm_kv_key, prewarm_kv_key};
use trogon_sandbox_protocol::{legacy_tenant_scoped_kv_prefix, tenant_scoped_kv_prefix};

use crate::constants::{PREWARM_CPU_SECONDS, PREWARM_LEASE_TTL_SECONDS, PREWARM_MEMORY_MIB};
use crate::core::BrokerCore;
use crate::generated_ulid::generated_ulid;
pub use crate::prewarm_error::PrewarmError;
pub use crate::prewarm_store_error::PrewarmStoreError;
use crate::snapshot_tracker::LiveSnapshotStore;
use crate::worker_registry::WorkerRegistry;

mod in_memory_prewarm_store;
mod nats_kv_prewarm_store;
mod prewarm_controller;
mod prewarm_request;
mod prewarm_state;
mod prewarm_store;

pub use in_memory_prewarm_store::InMemoryPrewarmStore;
pub use nats_kv_prewarm_store::NatsKvPrewarmStore;
pub use prewarm_controller::{PrewarmController, lru_budget_filter, run_prewarm_controller, warm_pool_count};
pub use prewarm_request::{PrewarmRequestError, build_prewarm_request};
pub use prewarm_state::PrewarmState;
pub use prewarm_store::PrewarmStore;

pub use crate::constants::{PREWARM_KV_TTL, PREWARM_TICK};

#[cfg(test)]
mod tests;
