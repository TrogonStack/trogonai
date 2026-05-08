use async_nats::jetstream::{
    self, ErrorCode,
    context::{
        CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind,
    },
    kv, stream,
};

pub const RUBRICS_BUCKET: &str = "EVALUATION_RUBRICS";
pub const RESULTS_BUCKET: &str = "EVALUATION_RESULTS";
pub const EVALUATIONS_STREAM: &str = "SESSION_EVALUATIONS";
pub const EVALUATIONS_SUBJECT: &str = "sessions.evaluate.>";

/// Provision the rubrics KV bucket.
pub async fn provision_rubrics_kv(
    js: &jetstream::Context,
) -> Result<kv::Store, Box<dyn std::error::Error + Send + Sync>> {
    provision_kv(
        js,
        kv::Config {
            bucket: RUBRICS_BUCKET.to_string(),
            history: 1,
            storage: stream::StorageType::File,
            ..Default::default()
        },
    )
    .await
}

/// Provision the evaluation results KV bucket.
pub async fn provision_results_kv(
    js: &jetstream::Context,
) -> Result<kv::Store, Box<dyn std::error::Error + Send + Sync>> {
    provision_kv(
        js,
        kv::Config {
            bucket: RESULTS_BUCKET.to_string(),
            history: 1,
            storage: stream::StorageType::File,
            ..Default::default()
        },
    )
    .await
}

/// Provision the `SESSION_EVALUATIONS` JetStream stream.
pub async fn provision_stream(
    js: &jetstream::Context,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    js.get_or_create_stream(stream::Config {
        name: EVALUATIONS_STREAM.to_string(),
        subjects: vec![EVALUATIONS_SUBJECT.to_string()],
        storage: stream::StorageType::File,
        retention: stream::RetentionPolicy::WorkQueue,
        ..Default::default()
    })
    .await
    .map(|_| ())
    .map_err(|e| e.to_string().into())
}

async fn provision_kv(
    js: &jetstream::Context,
    config: kv::Config,
) -> Result<kv::Store, Box<dyn std::error::Error + Send + Sync>> {
    let bucket = config.bucket.clone();
    match js.create_key_value(config).await {
        Ok(store) => Ok(store),
        Err(e) if is_already_exists(&e) => js
            .get_key_value(&bucket)
            .await
            .map_err(|e| format!("failed to open existing bucket {bucket}: {e}").into()),
        Err(e) => Err(e.to_string().into()),
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
