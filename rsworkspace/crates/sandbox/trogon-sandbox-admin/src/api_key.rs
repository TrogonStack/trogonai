use async_nats::jetstream::kv;
use futures::StreamExt;
use trogon_sandbox_auth::{ApiKeyRecord, BearerToken, NatsKvApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};

use crate::constants::RAW_TOKEN_LEN;
use crate::{AdminError, Console};

/// Mints a key, printing the token once.
pub async fn mint(
    store: &NatsKvApiKeyStore,
    tenant: TenantId,
    service_account: ServiceAccountId,
    name: &str,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let token = store
        .mint(tenant, service_account, name)
        .await
        .map_err(AdminError::step("mint api key"))?;
    console.out(token.expose_secret())?;
    console.err("store this token securely; it cannot be retrieved later")
}

/// Lists stored keys by hash, optionally for one tenant. A record that does not parse is an error naming its key, so a
/// damaged store is never listed as if it were complete.
pub async fn list(
    store: &NatsKvApiKeyStore,
    filter_tenant: Option<&TenantId>,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let mut keys = store.kv().keys().await.map_err(AdminError::step("list keys"))?;
    while let Some(key) = keys.next().await {
        let key = key.map_err(AdminError::step("iterate key"))?;
        let Some(bytes) = store.kv().get(&key).await.map_err(AdminError::step("read key"))? else {
            continue;
        };
        let record =
            serde_json::from_slice::<ApiKeyRecord>(&bytes).map_err(|source| AdminError::UndecodableApiKey {
                key: key.clone(),
                source,
            })?;
        if filter_tenant.is_some_and(|tenant| record.tenant() != tenant) {
            continue;
        }
        let created_at = record
            .created_at()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        console.out(format_args!(
            "{key}\t{}\t{}\t{}\t{created_at}",
            record.tenant(),
            record.service_account(),
            record.name()
        ))?;
    }
    Ok(())
}

/// Revokes a key given as its raw token or as its stored hash.
pub async fn revoke(
    store: &NatsKvApiKeyStore,
    token_or_hash: &str,
    console: &mut Console<'_>,
) -> Result<(), AdminError> {
    let key_hash = key_hash_of(token_or_hash)?;
    revoke_key(store.kv(), &key_hash, console).await
}

async fn revoke_key(kv: &kv::Store, key_hash: &str, console: &mut Console<'_>) -> Result<(), AdminError> {
    if kv.get(key_hash).await.map_err(AdminError::step("read key"))?.is_none() {
        return console.out("no such key");
    }
    kv.delete(key_hash).await.map_err(AdminError::step("delete key"))?;
    console.out("revoked")
}

/// The stored hash a raw token or an already hashed key maps to.
pub fn key_hash_of(token_or_hash: &str) -> Result<String, AdminError> {
    if is_raw_token(token_or_hash) {
        let token = BearerToken::new(token_or_hash).map_err(AdminError::step("read token"))?;
        Ok(token.hash().as_str().to_string())
    } else {
        Ok(token_or_hash.to_string())
    }
}

/// A raw token is 64 hex characters.
pub fn is_raw_token(candidate: &str) -> bool {
    candidate.len() == RAW_TOKEN_LEN && candidate.chars().all(|c| c.is_ascii_hexdigit())
}
