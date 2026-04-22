use std::time::Duration;

use async_nats::jetstream::{
    self, ErrorCode,
    context::{
        CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind,
        KeyValueError,
    },
    kv, stream,
};

use crate::error::RegistryError;

/// Name of the NATS KV bucket used for agent registration.
pub const BUCKET_NAME: &str = "AGENT_REGISTRY";

/// How often agents must call [`crate::Registry::refresh`] to stay visible.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// How long before an entry that has not been refreshed is automatically removed.
pub const ENTRY_TTL: Duration = Duration::from_secs(30);

/// Create or open the `AGENT_REGISTRY` KV bucket, returning the store handle.
///
/// Safe to call multiple times — if the bucket already exists the call falls
/// back to opening the existing one.
pub async fn provision(js: &jetstream::Context) -> Result<kv::Store, RegistryError> {
    match js.create_key_value(bucket_config()).await {
        Ok(store) => Ok(store),
        Err(e) if is_already_exists(&e) => js
            .get_key_value(BUCKET_NAME)
            .await
            .map_err(format_get_error),
        Err(e) => Err(RegistryError::Provision(e.to_string())),
    }
}

fn bucket_config() -> kv::Config {
    kv::Config {
        bucket: BUCKET_NAME.to_string(),
        // Keep only the latest revision per key — we only need current state.
        history: 1,
        // Entries expire automatically after ENTRY_TTL if not refreshed.
        max_age: ENTRY_TTL,
        // Memory storage: agents re-register after server restarts anyway,
        // and the registry is inherently ephemeral.
        storage: stream::StorageType::Memory,
        ..Default::default()
    }
}

fn is_already_exists(error: &CreateKeyValueError) -> bool {
    error.kind() == CreateKeyValueErrorKind::BucketCreate
        && std::error::Error::source(error)
            .and_then(|s| s.downcast_ref::<CreateStreamError>())
            .is_some_and(|s| {
                matches!(
                    s.kind(),
                    CreateStreamErrorKind::JetStream(ref j)
                        if j.error_code() == ErrorCode::STREAM_NAME_EXIST
                )
            })
}

fn format_get_error(e: KeyValueError) -> RegistryError {
    RegistryError::Provision(format!("failed to open existing registry bucket: {e}"))
}
