use async_nats::jetstream::{
    self, ErrorCode,
    context::{CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind},
    kv, stream,
};

use crate::error::CatalogClientError;

pub const BUCKET_NAME: &str = "MODEL_CATALOG";
pub const CATALOG_KEY: &str = "snapshot";
pub const PROVIDERS_KEY: &str = "providers";

/// Create or open the `MODEL_CATALOG` KV bucket.
pub async fn provision(js: &jetstream::Context) -> Result<kv::Store, CatalogClientError> {
    match js.create_key_value(bucket_config()).await {
        Ok(store) => Ok(store),
        Err(e) if is_already_exists(&e) => js
            .get_key_value(BUCKET_NAME)
            .await
            .map_err(|e| CatalogClientError::Provision(e.to_string())),
        Err(e) => Err(CatalogClientError::Provision(e.to_string())),
    }
}

fn bucket_config() -> kv::Config {
    kv::Config {
        bucket: BUCKET_NAME.to_string(),
        history: 1,
        storage: stream::StorageType::File,
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
