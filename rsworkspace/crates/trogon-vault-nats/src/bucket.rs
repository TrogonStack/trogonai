use async_nats::jetstream::{self, kv, stream::StorageType};

use crate::error::{NatsKvVaultError, NatsResult as _};

/// Create or open the KV bucket for `vault_name`.
///
/// Bucket name: `vault_{vault_name}` (e.g. `vault_prod`, `vault_staging`).
///
/// Settings:
/// - `history = 2`      — keeps current + previous version for grace-period rotation
/// - `storage = File`   — durable across NATS restarts
/// - `num_replicas`     — set to 1 for single-node, 3 for HA clusters
///
/// NOTE: `sync_interval = "always"` (recommended by Jepsen NATS 2025 findings for
/// maximum write durability) is not exposed in async-nats 0.47's `kv::Config`.
/// Configure it at the NATS server level (`jetstream { sync_interval: "always" }`)
/// for production deployments.
///
/// Idempotent: calling this multiple times with the same name is safe.
pub async fn ensure_vault_bucket(
    js: &jetstream::Context,
    vault_name: &str,
    num_replicas: usize,
) -> Result<kv::Store, NatsKvVaultError> {
    js.create_or_update_key_value(kv::Config {
        bucket:       format!("vault_{vault_name}"),
        history:      2,
        storage:      StorageType::File,
        num_replicas,
        ..Default::default()
    })
    .await
    .nats_err()
}
