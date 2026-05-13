use async_nats::jetstream::{
    self, ErrorCode,
    context::{CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind, KeyValueError},
    kv,
};

pub const KV_BUCKET: &str = "WEBHOOK_SUBSCRIPTIONS";

/// Create or open the `WEBHOOK_SUBSCRIPTIONS` KV bucket.
///
/// Safe to call multiple times — falls back to opening the existing bucket
/// if it already exists.
pub async fn provision(
    js: &jetstream::Context,
) -> Result<kv::Store, Box<dyn std::error::Error + Send + Sync>> {
    match js
        .create_key_value(kv::Config {
            bucket: KV_BUCKET.to_string(),
            history: 1,
            ..Default::default()
        })
        .await
    {
        Ok(store) => Ok(store),
        Err(e) if is_already_exists(&e) => {
            js.get_key_value(KV_BUCKET).await.map_err(format_get_err)
        }
        Err(e) => Err(Box::new(e)),
    }
}

fn is_already_exists(e: &CreateKeyValueError) -> bool {
    e.kind() == CreateKeyValueErrorKind::BucketCreate
        && std::error::Error::source(e)
            .and_then(|s| s.downcast_ref::<CreateStreamError>())
            .is_some_and(|s| {
                matches!(
                    s.kind(),
                    CreateStreamErrorKind::JetStream(ref j)
                        if j.error_code() == ErrorCode::STREAM_NAME_EXIST
                )
            })
}

fn format_get_err(e: KeyValueError) -> Box<dyn std::error::Error + Send + Sync> {
    format!("failed to open existing bucket: {e}").into()
}
