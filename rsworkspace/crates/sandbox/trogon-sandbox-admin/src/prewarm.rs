use async_nats::jetstream;
use trogon_sandbox_broker::prewarm::{NatsKvPrewarmStore, PrewarmState, PrewarmStore};
use trogon_sandbox_contract::{Digest, PrewarmSpec, ProviderKind, TenantId};

use crate::{AdminError, Console};

pub async fn set(
    js: jetstream::Context,
    tenant: &TenantId,
    digest: &Digest,
    pool_size: u32,
    providers: Vec<ProviderKind>,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let state = PrewarmState::new(tenant.clone(), digest.clone(), PrewarmSpec { pool_size, providers });
    let store = open(js).await?;
    store
        .put(tenant, digest, &state)
        .await
        .map_err(AdminError::step("store prewarm policy"))?;
    console.out_json(&state)
}

pub async fn show(js: jetstream::Context, tenant: &TenantId, console: &mut Console<'_>) -> Result<(), AdminError> {
    let entries = open(js)
        .await?
        .list_for_tenant(tenant)
        .await
        .map_err(AdminError::step("list prewarm policies"))?;
    if entries.is_empty() {
        return console.out(format_args!("no prewarm policies for tenant '{tenant}'"));
    }
    for (digest, state) in entries {
        let json = serde_json::to_string(&state).map_err(AdminError::Encode)?;
        console.out(format_args!("{digest} {json}"))?;
    }
    Ok(())
}

pub async fn clear(
    js: jetstream::Context,
    tenant: &TenantId,
    digest: &Digest,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let store = open(js).await?;
    let found = store
        .get(tenant, digest)
        .await
        .map_err(AdminError::step("read prewarm policy"))?;
    if found.is_none() {
        return console.out("no such prewarm policy");
    }
    store
        .delete(tenant, digest)
        .await
        .map_err(AdminError::step("delete prewarm policy"))?;
    console.out("cleared")
}

async fn open(js: jetstream::Context) -> Result<NatsKvPrewarmStore, AdminError> {
    NatsKvPrewarmStore::new(js)
        .await
        .map_err(AdminError::step("open prewarm store"))
}
