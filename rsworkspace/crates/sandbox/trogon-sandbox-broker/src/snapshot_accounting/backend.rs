use std::collections::HashMap;

use async_nats::jetstream::{self, kv};
use tokio::sync::Mutex;
use trogon_sandbox_contract::TenantId;

use super::tenant_artifacts::TenantArtifacts;

pub(super) enum Backend {
    Memory(Mutex<HashMap<TenantId, TenantArtifacts>>),
    Nats {
        kv: Box<kv::Store>,
        js: Box<jetstream::Context>,
    },
}
