use std::time::Duration;

use async_nats::jetstream::{self, context, kv};
use trogon_decider_nats::JetStreamStore;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, SnapshotRead, SnapshotWrite, StreamAppend, StreamRead, WriteSnapshotRequest,
    WriteSnapshotResponse,
};
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus, is_create_key_value_already_exists,
};

use super::credential_lifecycle_stream::{
    CredentialLifecycleEventSubjectResolver, CredentialLifecycleKey, CredentialLifecycleStreamId, stream_config,
};

pub(crate) const CREDENTIAL_LIFECYCLE_SNAPSHOT_BUCKET: &str = "GATEWAY_CREDENTIAL_LIFECYCLE_SNAPSHOTS";

#[derive(Clone)]
pub(crate) struct CredentialLifecycleEventStore {
    inner: JetStreamStore<CredentialLifecycleEventSubjectResolver>,
}

impl CredentialLifecycleEventStore {
    fn new(inner: JetStreamStore<CredentialLifecycleEventSubjectResolver>) -> Self {
        Self { inner }
    }

    pub(crate) fn as_jetstream(&self) -> &jetstream::Context {
        self.inner.as_jetstream()
    }

    pub(crate) fn events_stream(&self) -> &jetstream::stream::Stream {
        self.inner.events_stream()
    }
}

impl StreamRead<str> for CredentialLifecycleEventStore {
    type Error = <JetStreamStore<CredentialLifecycleEventSubjectResolver> as StreamRead<str>>::Error;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await
    }
}

impl StreamAppend<str> for CredentialLifecycleEventStore {
    type Error = <JetStreamStore<CredentialLifecycleEventSubjectResolver> as StreamAppend<str>>::Error;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        self.inner.append_stream(request).await
    }
}

impl<Payload> SnapshotRead<Payload, str> for CredentialLifecycleEventStore
where
    JetStreamStore<CredentialLifecycleEventSubjectResolver>: SnapshotRead<Payload, str>,
{
    type Error = <JetStreamStore<CredentialLifecycleEventSubjectResolver> as SnapshotRead<Payload, str>>::Error;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        let snapshot_id = credential_lifecycle_snapshot_id(request.snapshot_id);
        self.inner
            .read_snapshot(ReadSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
            })
            .await
    }
}

impl<Payload> SnapshotWrite<Payload, str> for CredentialLifecycleEventStore
where
    JetStreamStore<CredentialLifecycleEventSubjectResolver>: SnapshotWrite<Payload, str>,
    Payload: Send,
{
    type Error = <JetStreamStore<CredentialLifecycleEventSubjectResolver> as SnapshotWrite<Payload, str>>::Error;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let snapshot_id = credential_lifecycle_snapshot_id(request.snapshot_id);
        self.inner
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
                snapshot: request.snapshot,
            })
            .await
    }
}

fn credential_lifecycle_snapshot_id(stream_id: &str) -> String {
    let key = CredentialLifecycleKey::for_stream(&CredentialLifecycleStreamId::from(stream_id));
    format!("credential.{}", key.simple())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialLifecycleEventStoreError {
    #[error("failed to create credential lifecycle event stream: {0}")]
    CreateEventStream(#[source] Box<context::CreateStreamError>),
    #[error("failed to create credential lifecycle snapshot bucket: {0}")]
    CreateSnapshotBucket(#[source] Box<context::CreateKeyValueError>),
    #[error("failed to open existing credential lifecycle snapshot bucket: {0}")]
    OpenSnapshotBucket(#[source] Box<context::KeyValueError>),
    #[error("failed to inspect credential lifecycle snapshot bucket: {0}")]
    InspectSnapshotBucket(#[source] kv::StatusError),
    #[error("{source}")]
    IncompatibleSnapshotBucket {
        #[source]
        source: IncompatibleCredentialLifecycleSnapshotBucket,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "credential lifecycle snapshot bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub(crate) struct IncompatibleCredentialLifecycleSnapshotBucket {
    expected_history: i64,
    actual_history: i64,
    expected_max_age: Duration,
    actual_max_age: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnapshotBucketSettings {
    history: i64,
    max_age: Duration,
}

impl SnapshotBucketSettings {
    const EXPECTED: Self = Self {
        history: 1,
        max_age: Duration::ZERO,
    };
}

pub(crate) async fn open(
    context: jetstream::Context,
) -> Result<CredentialLifecycleEventStore, CredentialLifecycleEventStoreError> {
    let events_stream = context
        .get_or_create_stream(stream_config())
        .await
        .map_err(|source| CredentialLifecycleEventStoreError::CreateEventStream(Box::new(source)))?;
    let snapshot_bucket = provision_snapshot_bucket::<_, kv::Store>(&context).await?;

    Ok(CredentialLifecycleEventStore::new(
        JetStreamStore::builder(context, events_stream, snapshot_bucket)
            .with_subject_resolver(CredentialLifecycleEventSubjectResolver),
    ))
}

pub(crate) async fn provision_snapshot_bucket<C, S>(client: &C) -> Result<S, CredentialLifecycleEventStoreError>
where
    C: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let store = match client.create_key_value(snapshot_bucket_config()).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => client
            .get_key_value(CREDENTIAL_LIFECYCLE_SNAPSHOT_BUCKET)
            .await
            .map_err(|source| CredentialLifecycleEventStoreError::OpenSnapshotBucket(Box::new(source)))?,
        Err(source) => {
            return Err(CredentialLifecycleEventStoreError::CreateSnapshotBucket(Box::new(
                source,
            )));
        }
    };

    validate_snapshot_bucket(&store).await?;

    Ok(store)
}

pub(crate) fn snapshot_bucket_config() -> kv::Config {
    kv::Config {
        bucket: CREDENTIAL_LIFECYCLE_SNAPSHOT_BUCKET.to_string(),
        history: SnapshotBucketSettings::EXPECTED.history,
        max_age: SnapshotBucketSettings::EXPECTED.max_age,
        ..Default::default()
    }
}

async fn validate_snapshot_bucket<S>(store: &S) -> Result<(), CredentialLifecycleEventStoreError>
where
    S: JetStreamKeyValueStatus,
{
    let status = store
        .status()
        .await
        .map_err(CredentialLifecycleEventStoreError::InspectSnapshotBucket)?;
    let actual = snapshot_bucket_settings_from_status(&status);
    let expected = SnapshotBucketSettings::EXPECTED;

    if actual != expected {
        return Err(CredentialLifecycleEventStoreError::IncompatibleSnapshotBucket {
            source: IncompatibleCredentialLifecycleSnapshotBucket {
                expected_history: expected.history,
                actual_history: actual.history,
                expected_max_age: expected.max_age,
                actual_max_age: actual.max_age,
            },
        });
    }

    Ok(())
}

fn snapshot_bucket_settings_from_status(status: &kv::bucket::Status) -> SnapshotBucketSettings {
    SnapshotBucketSettings {
        history: status.history(),
        max_age: status.max_age(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::jetstream::context::{CreateKeyValueErrorKind, KeyValueErrorKind};
    use trogon_decider_nats::snapshot_key;
    use trogon_nats::jetstream::{MockJetStreamKvClient, MockJetStreamKvStore};

    use crate::secret_store::credential_lifecycle::CredentialLifecycleState;

    #[test]
    fn snapshot_id_uses_nats_safe_deterministic_key() {
        let snapshot_id = credential_lifecycle_snapshot_id("openbao:tenant-1:github/primary:webhook_secret");

        assert!(snapshot_id.starts_with("credential."));
        assert_eq!(snapshot_id.len(), "credential.".len() + 32);
        assert!(
            snapshot_id
                .strip_prefix("credential.")
                .unwrap()
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        );
    }

    #[test]
    fn snapshot_key_keeps_raw_credential_id_out_of_nats_kv_key() {
        let raw_stream_id = "openbao:tenant-1:github/primary:webhook_secret";
        let snapshot_id = credential_lifecycle_snapshot_id(raw_stream_id);
        let key = snapshot_key::<CredentialLifecycleState>(&snapshot_id).unwrap();

        assert!(
            key.starts_with("snapshots.data.trogonai.gateway.credentials.state.v1.CredentialLifecycleStateSnapshot.")
        );
        assert!(!key.contains(raw_stream_id));
        assert!(!key.contains(':'));
        assert!(!key.contains('/'));
        assert!(key.ends_with(&snapshot_id));
    }

    #[test]
    fn snapshot_bucket_config_matches_runtime_contract() {
        let config = snapshot_bucket_config();

        assert_eq!(config.bucket, CREDENTIAL_LIFECYCLE_SNAPSHOT_BUCKET);
        assert_eq!(config.history, 1);
        assert_eq!(config.max_age, Duration::ZERO);
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_uses_create_on_success() {
        let client = MockJetStreamKvClient::new();
        let store = MockJetStreamKvStore::new();
        client.set_create_result(store);

        let result = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client).await;

        assert!(result.is_ok());
        assert!(client.requested_buckets().is_empty());
        assert_eq!(client.create_configs()[0].bucket, CREDENTIAL_LIFECYCLE_SNAPSHOT_BUCKET);
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_falls_back_to_get_on_already_exists() {
        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        client.set_get_result(MockJetStreamKvStore::new());

        let result = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client).await;

        assert!(result.is_ok());
        assert_eq!(
            client.requested_buckets(),
            vec![CREDENTIAL_LIFECYCLE_SNAPSHOT_BUCKET.to_string()]
        );
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_surfaces_get_errors_after_already_exists() {
        let client = MockJetStreamKvClient::new();
        client.fail_create_already_exists();
        client.fail_get(KeyValueErrorKind::GetBucket);

        let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to open existing credential lifecycle snapshot bucket")
        );
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_surfaces_create_errors() {
        let client = MockJetStreamKvClient::new();
        client.fail_create(CreateKeyValueErrorKind::TimedOut);

        let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to create credential lifecycle snapshot bucket")
        );
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_surfaces_status_errors() {
        let client = MockJetStreamKvClient::new();
        let store = MockJetStreamKvStore::new();
        store.fail_status(kv::StatusErrorKind::TimedOut);
        client.set_create_result(store);

        let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to inspect credential lifecycle snapshot bucket")
        );
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_rejects_incompatible_history() {
        let client = MockJetStreamKvClient::new();
        let store = MockJetStreamKvStore::new();
        store.set_settings(2, Duration::ZERO, true, None);
        client.set_create_result(store);

        let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("credential lifecycle snapshot bucket is incompatible")
        );
    }

    #[tokio::test]
    async fn provision_snapshot_bucket_rejects_incompatible_max_age() {
        let client = MockJetStreamKvClient::new();
        let store = MockJetStreamKvStore::new();
        store.set_settings(1, Duration::from_secs(60), true, None);
        client.set_create_result(store);

        let error = provision_snapshot_bucket::<_, MockJetStreamKvStore>(&client)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("credential lifecycle snapshot bucket is incompatible")
        );
    }
}
