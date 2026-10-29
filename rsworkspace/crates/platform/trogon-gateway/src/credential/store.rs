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

use super::stream::{CredentialEventSubjectResolver, CredentialKey, CredentialStreamId, stream_config};

pub(crate) const CREDENTIAL_SNAPSHOT_BUCKET: &str = "GATEWAY_CREDENTIAL_SNAPSHOTS";

#[derive(Clone)]
pub(crate) struct CredentialEventStore {
    inner: JetStreamStore<CredentialEventSubjectResolver>,
}

impl CredentialEventStore {
    fn new(inner: JetStreamStore<CredentialEventSubjectResolver>) -> Self {
        Self { inner }
    }

    pub(crate) fn as_jetstream(&self) -> &jetstream::Context {
        self.inner.as_jetstream()
    }

    pub(crate) fn events_stream(&self) -> &jetstream::stream::Stream {
        self.inner.events_stream()
    }
}

impl StreamRead<str> for CredentialEventStore {
    type Error = <JetStreamStore<CredentialEventSubjectResolver> as StreamRead<str>>::Error;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        self.inner.read_stream(request).await
    }
}

impl StreamAppend<str> for CredentialEventStore {
    type Error = <JetStreamStore<CredentialEventSubjectResolver> as StreamAppend<str>>::Error;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        self.inner.append_stream(request).await
    }
}

impl<Payload> SnapshotRead<Payload, str> for CredentialEventStore
where
    JetStreamStore<CredentialEventSubjectResolver>: SnapshotRead<Payload, str>,
{
    type Error = <JetStreamStore<CredentialEventSubjectResolver> as SnapshotRead<Payload, str>>::Error;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        let snapshot_id = credential_snapshot_id(request.snapshot_id);
        self.inner
            .read_snapshot(ReadSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
            })
            .await
    }
}

impl<Payload> SnapshotWrite<Payload, str> for CredentialEventStore
where
    JetStreamStore<CredentialEventSubjectResolver>: SnapshotWrite<Payload, str>,
    Payload: Send,
{
    type Error = <JetStreamStore<CredentialEventSubjectResolver> as SnapshotWrite<Payload, str>>::Error;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        let snapshot_id = credential_snapshot_id(request.snapshot_id);
        self.inner
            .write_snapshot(WriteSnapshotRequest {
                snapshot_id: snapshot_id.as_str(),
                snapshot: request.snapshot,
            })
            .await
    }
}

fn credential_snapshot_id(stream_id: &str) -> String {
    let key = CredentialKey::for_stream(&CredentialStreamId::from(stream_id));
    format!("credential.{}", key.simple())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialEventStoreError {
    #[error("failed to create credential event stream: {0}")]
    CreateEventStream(#[source] Box<context::CreateStreamError>),
    #[error("failed to create credential snapshot bucket: {0}")]
    CreateSnapshotBucket(#[source] Box<context::CreateKeyValueError>),
    #[error("failed to open existing credential snapshot bucket: {0}")]
    OpenSnapshotBucket(#[source] Box<context::KeyValueError>),
    #[error("failed to inspect credential snapshot bucket: {0}")]
    InspectSnapshotBucket(#[source] kv::StatusError),
    #[error("{source}")]
    IncompatibleSnapshotBucket {
        #[source]
        source: IncompatibleCredentialSnapshotBucket,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "credential snapshot bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub(crate) struct IncompatibleCredentialSnapshotBucket {
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

pub(crate) async fn open(context: jetstream::Context) -> Result<CredentialEventStore, CredentialEventStoreError> {
    let events_stream = context
        .get_or_create_stream(stream_config())
        .await
        .map_err(|source| CredentialEventStoreError::CreateEventStream(Box::new(source)))?;
    let snapshot_bucket = provision_snapshot_bucket::<_, kv::Store>(&context).await?;

    Ok(CredentialEventStore::new(
        JetStreamStore::builder(context, events_stream, snapshot_bucket)
            .with_subject_resolver(CredentialEventSubjectResolver),
    ))
}

pub(crate) async fn provision_snapshot_bucket<C, S>(client: &C) -> Result<S, CredentialEventStoreError>
where
    C: JetStreamCreateKeyValue<Store = S> + JetStreamGetKeyValue<Store = S>,
    S: JetStreamKeyValueStatus,
{
    let store = match client.create_key_value(snapshot_bucket_config()).await {
        Ok(store) => store,
        Err(source) if is_create_key_value_already_exists(&source) => client
            .get_key_value(CREDENTIAL_SNAPSHOT_BUCKET)
            .await
            .map_err(|source| CredentialEventStoreError::OpenSnapshotBucket(Box::new(source)))?,
        Err(source) => {
            return Err(CredentialEventStoreError::CreateSnapshotBucket(Box::new(source)));
        }
    };

    validate_snapshot_bucket(&store).await?;

    Ok(store)
}

pub(crate) fn snapshot_bucket_config() -> kv::Config {
    kv::Config {
        bucket: CREDENTIAL_SNAPSHOT_BUCKET.to_string(),
        history: SnapshotBucketSettings::EXPECTED.history,
        max_age: SnapshotBucketSettings::EXPECTED.max_age,
        ..Default::default()
    }
}

async fn validate_snapshot_bucket<S>(store: &S) -> Result<(), CredentialEventStoreError>
where
    S: JetStreamKeyValueStatus,
{
    let status = store
        .status()
        .await
        .map_err(CredentialEventStoreError::InspectSnapshotBucket)?;
    let actual = snapshot_bucket_settings_from_status(&status);
    let expected = SnapshotBucketSettings::EXPECTED;

    if actual != expected {
        return Err(CredentialEventStoreError::IncompatibleSnapshotBucket {
            source: IncompatibleCredentialSnapshotBucket {
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
mod tests;
