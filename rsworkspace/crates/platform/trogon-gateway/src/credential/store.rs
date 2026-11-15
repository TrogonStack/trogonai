use std::time::Duration;

use async_nats::jetstream::{self, context, kv};
use trogon_credential_aggregate::{CredentialEventSubjectResolver, CredentialEventSubjectResolverError};
use trogon_credential_domain::CredentialKey;
use trogon_decider_nats::{JetStreamStore, JetStreamStoreBuilder, JetStreamStoreError};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite,
    StreamAppend, StreamRead, WriteSnapshotRequest, WriteSnapshotResponse,
};
use trogon_nats::jetstream::{
    JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus, is_create_key_value_already_exists,
};
use uuid::Uuid;

use super::constants::{CREDENTIAL_NAMESPACE, CREDENTIAL_SNAPSHOT_BUCKET};
use super::ensure_event_stream::ensure_credential_event_stream;
use super::ensure_event_stream_error::EnsureCredentialEventStreamError;

type CredentialStoreError<Payload = std::convert::Infallible, Type = std::convert::Infallible> =
    JetStreamStoreError<CredentialEventSubjectResolverError, Payload, Type>;

/// Event store whose every operation runs inside the stream's own project.
///
/// The ADR#0027 scope is fixed when a resolver is built, so the store builds
/// one per call from the project that travels in the [`CredentialKey`].
/// Building is a handful of handle clones, and it is what lets the shared
/// JetStream stream and snapshot bucket serve every project while no call can
/// resolve outside the project it names.
#[derive(Clone)]
pub(crate) struct CredentialEventStore {
    builder: JetStreamStoreBuilder,
    js: jetstream::Context,
    events_stream: jetstream::stream::Stream,
    snapshot_bucket: kv::Store,
}

impl CredentialEventStore {
    fn new(js: jetstream::Context, events_stream: jetstream::stream::Stream, snapshot_bucket: kv::Store) -> Self {
        Self {
            builder: JetStreamStore::builder(js.clone(), events_stream.clone(), snapshot_bucket.clone()),
            js,
            events_stream,
            snapshot_bucket,
        }
    }

    pub(crate) fn as_jetstream(&self) -> &jetstream::Context {
        &self.js
    }

    pub(crate) fn events_stream(&self) -> &jetstream::stream::Stream {
        &self.events_stream
    }

    fn project_store(
        &self,
        stream_id: &CredentialKey,
    ) -> Result<JetStreamStore<CredentialEventSubjectResolver>, CredentialStoreError> {
        let resolver = CredentialEventSubjectResolver::for_owner(stream_id.owner_id())
            .map_err(JetStreamStoreError::ResolveSubject)?;
        Ok(self.builder.clone().with_subject_resolver(resolver))
    }
}

impl StreamRead<CredentialKey> for CredentialEventStore {
    type Error = CredentialStoreError;

    async fn read_stream(
        &self,
        request: ReadStreamRequest<'_, CredentialKey>,
    ) -> Result<ReadStreamResponse, Self::Error> {
        self.project_store(request.stream_id)?.read_stream(request).await
    }
}

impl StreamAppend<CredentialKey> for CredentialEventStore {
    type Error = CredentialStoreError;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, CredentialKey>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        self.project_store(request.stream_id)?.append_stream(request).await
    }
}

impl<Payload> SnapshotRead<Payload, CredentialKey> for CredentialEventStore
where
    Payload: SnapshotPayloadDecode + SnapshotType + Send,
    <Payload as SnapshotPayloadDecode>::Error: std::error::Error + Send + Sync + 'static,
    <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = CredentialStoreError<<Payload as SnapshotPayloadDecode>::Error, <Payload as SnapshotType>::Error>;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, CredentialKey>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        trogon_decider_nats::read_snapshot(&self.snapshot_bucket, &credential_snapshot_id(request.snapshot_id))
            .await
            .map(|snapshot| ReadSnapshotResponse { snapshot })
            .map_err(JetStreamStoreError::Snapshot)
    }
}

impl<Payload> SnapshotWrite<Payload, CredentialKey> for CredentialEventStore
where
    Payload: SnapshotPayloadEncode + SnapshotType + Send,
    <Payload as SnapshotPayloadEncode>::Error: std::error::Error + Send + Sync + 'static,
    <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = CredentialStoreError<<Payload as SnapshotPayloadEncode>::Error, <Payload as SnapshotType>::Error>;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, CredentialKey>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        trogon_decider_nats::write_snapshot(
            &self.snapshot_bucket,
            &credential_snapshot_id(request.snapshot_id),
            request.snapshot,
        )
        .await
        .map(|()| WriteSnapshotResponse)
        .map_err(JetStreamStoreError::Snapshot)
    }
}

/// The bucket key of a credential's snapshot.
///
/// Key/Value keys cannot carry every character a project id may, so the key is
/// a digest of the whole stream id, project included. Two projects can never
/// share a key, and the key reveals neither the project nor the credential.
fn credential_snapshot_id(stream_id: &CredentialKey) -> String {
    let key = Uuid::new_v5(&CREDENTIAL_NAMESPACE, stream_id.to_string().as_bytes());
    format!("credential.{}", key.as_simple())
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CredentialEventStoreError {
    #[error("failed to ensure the credential event stream: {0}")]
    EnsureEventStream(#[source] Box<EnsureCredentialEventStreamError>),
    #[error("failed to create credential snapshot bucket: {0}")]
    CreateSnapshotBucket(#[source] Box<context::CreateKeyValueError>),
    #[error("failed to open existing credential snapshot bucket: {0}")]
    OpenSnapshotBucket(#[source] Box<context::KeyValueError>),
    #[error("failed to inspect credential snapshot bucket: {0}")]
    InspectSnapshotBucket(#[source] kv::StatusError),
    #[error("{source}")]
    IncompatibleSnapshotBucket {
        #[source]
        source: IncompatibleCredentialSnapshotBucketError,
    },
}

#[derive(Debug, thiserror::Error)]
#[error(
    "credential snapshot bucket is incompatible: expected history {expected_history}, got {actual_history}; expected max age {expected_max_age:?}, got {actual_max_age:?}"
)]
pub(crate) struct IncompatibleCredentialSnapshotBucketError {
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
    let events_stream = ensure_credential_event_stream(&context)
        .await
        .map_err(|source| CredentialEventStoreError::EnsureEventStream(Box::new(source)))?;
    let snapshot_bucket = provision_snapshot_bucket::<_, kv::Store>(&context).await?;

    Ok(CredentialEventStore::new(context, events_stream, snapshot_bucket))
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
            source: IncompatibleCredentialSnapshotBucketError {
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
