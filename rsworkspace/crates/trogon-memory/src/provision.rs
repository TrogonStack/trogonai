use async_nats::jetstream::{
    self, ErrorCode,
    context::{
        CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind,
        KeyValueError,
    },
    kv, stream,
};

pub const MEMORIES_BUCKET: &str = "SESSION_MEMORIES";
pub const DREAMS_STREAM: &str = "SESSION_DREAMS";
pub const DREAMS_SUBJECT: &str = "sessions.dream.>";

/// Create or open the `SESSION_MEMORIES` KV bucket.
pub async fn provision_kv(
    js: &jetstream::Context,
) -> Result<kv::Store, Box<dyn std::error::Error + Send + Sync>> {
    match js
        .create_key_value(kv::Config {
            bucket: MEMORIES_BUCKET.to_string(),
            history: 1,
            storage: stream::StorageType::File,
            ..Default::default()
        })
        .await
    {
        Ok(store) => Ok(store),
        Err(e) if is_already_exists(&e) => js
            .get_key_value(MEMORIES_BUCKET)
            .await
            .map_err(|e| format!("failed to open existing memories bucket: {e}").into()),
        Err(e) => Err(e.to_string().into()),
    }
}

/// Create or ensure the `SESSION_DREAMS` JetStream stream exists.
pub async fn provision_stream(
    js: &jetstream::Context,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    js.get_or_create_stream(stream::Config {
        name: DREAMS_STREAM.to_string(),
        subjects: vec![DREAMS_SUBJECT.to_string()],
        storage: stream::StorageType::File,
        retention: stream::RetentionPolicy::WorkQueue,
        ..Default::default()
    })
    .await
    .map(|_| ())
    .map_err(|e| e.to_string().into())
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

// suppress unused import warning for KeyValueError which is used in the error path
#[allow(dead_code)]
fn _use_kv_error(_: KeyValueError) {}
