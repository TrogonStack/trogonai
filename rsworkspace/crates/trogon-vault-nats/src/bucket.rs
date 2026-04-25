use async_nats::jetstream::{self, kv, stream::StorageType};

use crate::error::{NatsKvVaultError, NatsResult as _};

/// Create or open the KV bucket for `vault_name`.
///
/// Bucket name: `vault_{vault_name}` (e.g. `vault_prod`, `vault_staging`).
///
/// Settings:
/// - `history = 2` — keeps current + previous version for grace-period rotation
/// - `storage = File` — durable across NATS restarts
///
/// Idempotent: calling this multiple times with the same name is safe.
pub async fn ensure_vault_bucket(
    js: &jetstream::Context,
    vault_name: &str,
) -> Result<kv::Store, NatsKvVaultError> {
    js.create_or_update_key_value(kv::Config {
        bucket:  format!("vault_{vault_name}"),
        history: 2,
        storage: StorageType::File,
        ..Default::default()
    })
    .await
    .nats_err()
}
