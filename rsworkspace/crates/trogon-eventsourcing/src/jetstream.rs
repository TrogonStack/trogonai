use std::fmt;
use std::marker::PhantomData;

use async_nats::jetstream::{self, kv};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    AppendOutcome, EventData, NonEmpty, Snapshot, SnapshotChange, SnapshotRead,
    SnapshotStoreConfig, SnapshotStoreError, SnapshotWrite, StreamAppend, StreamRead,
    StreamReadResult, StreamState, StreamStoreError, append_stream, load_snapshot,
    persist_snapshot_change, read_stream_from,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectState {
    pub subject: String,
    pub current_version: Option<u64>,
}

pub trait StreamSubjectResolver<StreamId: ?Sized>: Send + Sync + Clone + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<SubjectState, Self::Error>> + Send;
}

pub trait AppendProjector<StreamId: ?Sized>: Send + Sync + Clone + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn project_appended(
        &self,
        snapshot_bucket: &kv::Store,
        stream_id: &StreamId,
        events: &[EventData],
        next_expected_version: u64,
    ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send;
}

#[derive(Debug)]
pub struct NoAppendProjection<Error>(PhantomData<fn() -> Error>);

impl<Error> Default for NoAppendProjection<Error> {
    fn default() -> Self {
        Self(PhantomData)
    }
}

impl<Error> Clone for NoAppendProjection<Error> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<Error> Copy for NoAppendProjection<Error> {}

impl<StreamId: ?Sized + Sync, Error> AppendProjector<StreamId> for NoAppendProjection<Error>
where
    Error: std::error::Error + Send + Sync + 'static,
{
    type Error = Error;

    async fn project_appended(
        &self,
        _snapshot_bucket: &kv::Store,
        _stream_id: &StreamId,
        _events: &[EventData],
        _next_expected_version: u64,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[derive(Debug)]
pub enum JetStreamStoreError<Error> {
    ResolveSubject(Error),
    ReadStream(StreamStoreError),
    AppendStream(StreamStoreError),
    ProjectAppend(Error),
    Snapshot(SnapshotStoreError),
    Codec(serde_json::Error),
    OptimisticConcurrencyConflict {
        stream_id: String,
        expected: StreamState,
        current_version: Option<u64>,
    },
}

impl<Error> fmt::Display for JetStreamStoreError<Error>
where
    Error: std::error::Error + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ResolveSubject(source) => {
                write!(f, "failed to resolve stream subject state: {source}")
            }
            Self::ReadStream(source) => write!(f, "failed to read stream events: {source}"),
            Self::AppendStream(source) => write!(f, "failed to append stream events: {source}"),
            Self::ProjectAppend(source) => {
                write!(f, "failed to project appended stream events: {source}")
            }
            Self::Snapshot(source) => write!(f, "failed to access snapshots: {source}"),
            Self::Codec(source) => write!(f, "codec error: {source}"),
            Self::OptimisticConcurrencyConflict {
                stream_id,
                expected,
                current_version,
            } => match current_version {
                Some(current_version) => write!(
                    f,
                    "OCC conflict for stream '{stream_id}': expected {expected:?}, current version is {current_version}"
                ),
                None => write!(
                    f,
                    "OCC conflict for stream '{stream_id}': expected {expected:?}, stream has no current version"
                ),
            },
        }
    }
}

impl<Error> std::error::Error for JetStreamStoreError<Error>
where
    Error: std::error::Error + Send + Sync + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ResolveSubject(source) | Self::ProjectAppend(source) => Some(source),
            Self::ReadStream(source) | Self::AppendStream(source) => Some(source),
            Self::Snapshot(source) => Some(source),
            Self::Codec(source) => Some(source),
            Self::OptimisticConcurrencyConflict { .. } => None,
        }
    }
}

impl<Error> From<serde_json::Error> for JetStreamStoreError<Error> {
    fn from(value: serde_json::Error) -> Self {
        Self::Codec(value)
    }
}

#[derive(Clone)]
pub struct JetStreamStore<Resolver, Projector> {
    js: jetstream::Context,
    events_stream: jetstream::stream::Stream,
    snapshot_bucket: kv::Store,
    subject_resolver: Resolver,
    append_projector: Projector,
}

impl<Resolver, Projector> JetStreamStore<Resolver, Projector> {
    pub fn new(
        js: jetstream::Context,
        events_stream: jetstream::stream::Stream,
        snapshot_bucket: kv::Store,
        subject_resolver: Resolver,
        append_projector: Projector,
    ) -> Self {
        Self {
            js,
            events_stream,
            snapshot_bucket,
            subject_resolver,
            append_projector,
        }
    }

    pub fn as_jetstream(&self) -> &jetstream::Context {
        &self.js
    }

    pub fn events_stream(&self) -> &jetstream::stream::Stream {
        &self.events_stream
    }

    pub fn snapshot_bucket(&self) -> &kv::Store {
        &self.snapshot_bucket
    }

    pub async fn read_events_from<StreamId>(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> Result<StreamReadResult, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
        Resolver: StreamSubjectResolver<StreamId>,
        Projector: AppendProjector<StreamId, Error = Resolver::Error>,
    {
        let current_version = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?
            .current_version;
        let events = read_stream_from(self.events_stream(), from_sequence)
            .await
            .map_err(JetStreamStoreError::ReadStream)?
            .into_iter()
            .filter(|event| event.stream_id() == stream_id.as_ref())
            .collect();

        Ok(StreamReadResult {
            current_version,
            events,
        })
    }

    pub async fn append_to_stream<StreamId>(
        &self,
        stream_id: &StreamId,
        expected_state: StreamState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
        Resolver: StreamSubjectResolver<StreamId>,
        Projector: AppendProjector<StreamId, Error = Resolver::Error>,
    {
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        let current_version = subject_state.current_version;
        let expected_last_subject_sequence = match expected_state {
            StreamState::Any => current_version.unwrap_or(0),
            StreamState::StreamExists => current_version.ok_or_else(|| {
                JetStreamStoreError::OptimisticConcurrencyConflict {
                    stream_id: stream_id.to_string(),
                    expected: StreamState::StreamExists,
                    current_version,
                }
            })?,
            StreamState::NoStream => 0,
            StreamState::StreamRevision(version) => version,
        };

        append_stream(
            self.as_jetstream(),
            subject_state.subject,
            expected_last_subject_sequence,
            &events,
        )
        .await
        .map_err(|source| match source {
            StreamStoreError::WrongExpectedVersion => {
                JetStreamStoreError::OptimisticConcurrencyConflict {
                    stream_id: stream_id.to_string(),
                    expected: expected_state,
                    current_version,
                }
            }
            other => JetStreamStoreError::AppendStream(other),
        })?;

        let next_expected_version = current_version.unwrap_or(0) + events.len() as u64;
        self.append_projector
            .project_appended(
                self.snapshot_bucket(),
                stream_id,
                events.as_slice(),
                next_expected_version,
            )
            .await
            .map_err(JetStreamStoreError::ProjectAppend)?;

        Ok(AppendOutcome {
            next_expected_version,
        })
    }

    pub async fn load_snapshot_entry<StreamId, Payload>(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &StreamId,
    ) -> Result<Option<Snapshot<Payload>>, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + Send + Sync + ?Sized,
        Payload: Serialize + DeserializeOwned + Send,
        Resolver: StreamSubjectResolver<StreamId>,
        Projector: AppendProjector<StreamId, Error = Resolver::Error>,
    {
        load_snapshot(self.snapshot_bucket(), config, stream_id.as_ref())
            .await
            .map_err(JetStreamStoreError::Snapshot)
    }

    pub async fn save_snapshot_entry<StreamId, Payload>(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &StreamId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + Send + Sync + ?Sized,
        Payload: Serialize + DeserializeOwned + Send,
        Resolver: StreamSubjectResolver<StreamId>,
        Projector: AppendProjector<StreamId, Error = Resolver::Error>,
    {
        persist_snapshot_change(
            self.snapshot_bucket(),
            config,
            SnapshotChange::upsert(stream_id.as_ref(), snapshot),
        )
        .await
        .map_err(JetStreamStoreError::Snapshot)
    }
}

pub async fn subject_current_version(
    stream: &jetstream::stream::Stream,
    subject: &str,
) -> Result<Option<u64>, StreamStoreError> {
    match stream.get_last_raw_message_by_subject(subject).await {
        Ok(message) => Ok(Some(message.sequence)),
        Err(error)
            if matches!(
                error.kind(),
                async_nats::jetstream::stream::LastRawMessageErrorKind::NoMessageFound
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(StreamStoreError::Read {
            context: "failed to read latest subject message",
            source: Box::new(error),
        }),
    }
}

impl<StreamId, Resolver, Projector> StreamRead<StreamId> for JetStreamStore<Resolver, Projector>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
    Projector: AppendProjector<StreamId, Error = Resolver::Error>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_stream_from(
        &self,
        stream_id: &StreamId,
        from_sequence: u64,
    ) -> Result<StreamReadResult, Self::Error> {
        self.read_events_from(stream_id, from_sequence).await
    }
}

impl<StreamId, Resolver, Projector> StreamAppend<StreamId> for JetStreamStore<Resolver, Projector>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
    Projector: AppendProjector<StreamId, Error = Resolver::Error>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn append_events(
        &self,
        stream_id: &StreamId,
        expected_state: StreamState,
        events: NonEmpty<EventData>,
    ) -> Result<AppendOutcome, Self::Error> {
        self.append_to_stream(stream_id, expected_state, events)
            .await
    }
}

impl<StreamId, Payload, Resolver, Projector> SnapshotRead<Payload, StreamId>
    for JetStreamStore<Resolver, Projector>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + Send,
    Resolver: StreamSubjectResolver<StreamId>,
    Projector: AppendProjector<StreamId, Error = Resolver::Error>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn load_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &StreamId,
    ) -> Result<Option<Snapshot<Payload>>, Self::Error> {
        self.load_snapshot_entry(config, stream_id).await
    }
}

impl<StreamId, Payload, Resolver, Projector> SnapshotWrite<Payload, StreamId>
    for JetStreamStore<Resolver, Projector>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + Send,
    Resolver: StreamSubjectResolver<StreamId>,
    Projector: AppendProjector<StreamId, Error = Resolver::Error>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn save_snapshot(
        &self,
        config: SnapshotStoreConfig<'static>,
        stream_id: &StreamId,
        snapshot: Snapshot<Payload>,
    ) -> Result<(), Self::Error> {
        self.save_snapshot_entry(config, stream_id, snapshot).await
    }
}
