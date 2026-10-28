use super::*;
use async_nats::jetstream::context::{
    CreateKeyValueError, CreateKeyValueErrorKind, CreateStreamError, CreateStreamErrorKind,
};
use trogon_nats::test_support::CoreTestServer;

fn stream_exists_error() -> CreateStreamError {
    let source: jetstream::Error = serde_json::from_str(
        r#"{"code":400,"err_code":10058,"description":"stream name already in use with a different configuration"}"#,
    )
    .unwrap();

    CreateStreamError::new(CreateStreamErrorKind::JetStream(source))
}

#[test]
fn create_key_value_already_exists_matches_wrapped_stream_exists_error() {
    let error = CreateKeyValueError::with_source(CreateKeyValueErrorKind::BucketCreate, stream_exists_error());

    assert!(is_create_key_value_already_exists(&error));
}

#[tokio::test]
async fn disabled_jetstream_preserves_infrastructure_errors_during_provisioning() {
    let server = CoreTestServer::start().await;
    let js = jetstream::new(async_nats::connect(server.address()).await.unwrap());
    let snapshot = get_or_create_command_snapshot_bucket(&js).await.err().unwrap();
    assert!(matches!(snapshot, SchedulerError::Kv { .. }));
    assert!(std::error::Error::source(&snapshot).is_some());
    let events = get_or_create_events_stream(&js).await.err().unwrap();
    assert!(matches!(events, SchedulerError::Event { .. }));
    assert!(std::error::Error::source(&events).is_some());
    let schedules = crate::projections::storage::get_or_create_schedules_bucket(&js)
        .await
        .err()
        .unwrap();
    assert!(matches!(schedules, SchedulerError::Kv { .. }));
    assert!(std::error::Error::source(&schedules).is_some());
    let open = crate::projections::storage::open_schedules_bucket(&js)
        .await
        .err()
        .unwrap();
    assert!(matches!(open, SchedulerError::Kv { .. }));
    assert!(std::error::Error::source(&open).is_some());
}

#[derive(Clone)]
struct DisappearedResources;

impl JetStreamCreateKeyValue for DisappearedResources {
    type Store = kv::Store;

    async fn create_key_value(&self, _config: kv::Config) -> Result<Self::Store, CreateKeyValueError> {
        Err(CreateKeyValueError::with_source(
            CreateKeyValueErrorKind::BucketCreate,
            stream_exists_error(),
        ))
    }
}

impl JetStreamGetKeyValue for DisappearedResources {
    type Store = kv::Store;

    async fn get_key_value<T: Into<String> + Send>(
        &self,
        _bucket: T,
    ) -> Result<Self::Store, jetstream::context::KeyValueError> {
        Err(jetstream::context::KeyValueError::new(
            jetstream::context::KeyValueErrorKind::GetBucket,
        ))
    }
}

impl JetStreamGetStream for DisappearedResources {
    type Error = std::io::Error;
    type Stream = stream::Stream;

    async fn get_stream<T: AsRef<str> + Send>(&self, _name: T) -> Result<Self::Stream, Self::Error> {
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    }
}

#[tokio::test]
async fn bucket_removed_between_create_conflict_and_reopen_preserves_the_reopen_error() {
    let error = get_or_create(
        &DisappearedResources,
        kv::Config {
            bucket: COMMAND_SNAPSHOT_BUCKET.to_string(),
            ..Default::default()
        },
    )
    .await
    .err()
    .unwrap();
    let SchedulerError::Kv { source, .. } = error else {
        panic!("bucket reopening must report a key-value error");
    };
    assert_eq!(
        source
            .downcast_ref::<jetstream::context::KeyValueError>()
            .unwrap()
            .kind(),
        jetstream::context::KeyValueErrorKind::GetBucket
    );
}

#[tokio::test]
async fn stream_removed_between_create_conflict_and_reopen_preserves_the_reopen_error() {
    let error = created_events_stream(&DisappearedResources, Err(stream_exists_error()))
        .await
        .err()
        .unwrap();
    let SchedulerError::Event { source, .. } = error else {
        panic!("stream reopening must report an event error");
    };
    assert_eq!(
        source.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::NotFound
    );
}
