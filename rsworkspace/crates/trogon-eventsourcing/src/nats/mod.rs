pub mod snapshot_store;
pub mod stream_store;

use std::fmt;

use async_nats::jetstream::{self, kv};
use serde::{Serialize, de::DeserializeOwned};

use crate::nats::stream_store::{append_stream as append_subject_stream, read_subject_stream};
use crate::snapshot::{
    ReadSnapshotRequest, ReadSnapshotResponse, SnapshotType, WriteSnapshotRequest, WriteSnapshotResponse,
};
use crate::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, ReadStreamResponse, SnapshotRead,
    SnapshotWrite, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};
pub use snapshot_store::{
    NatsSnapshotConfig, SnapshotChange, SnapshotStoreError, checkpoint_key, list_snapshots, maybe_advance_checkpoint,
    persist_snapshot_change, read_checkpoint, read_snapshot, read_snapshot_map, snapshot_key, write_checkpoint,
    write_snapshot,
};
pub use stream_store::{
    StreamStoreError, TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range,
    record_stream_message,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectState {
    pub subject: String,
    pub current_position: Option<StreamPosition>,
}

pub trait StreamSubjectResolver<StreamId: ?Sized>: Send + Sync + Clone + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<SubjectState, Self::Error>> + Send;
}

#[derive(Debug)]
pub enum JetStreamStoreError<Error> {
    ResolveSubject(Error),
    ReadStream(StreamStoreError),
    AppendStream(StreamStoreError),
    Snapshot(SnapshotStoreError),
    Codec(Error),
    OptimisticConcurrencyConflict {
        stream_id: String,
        expected: StreamWritePrecondition,
        current_position: Option<StreamPosition>,
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
            Self::Snapshot(source) => write!(f, "failed to access snapshots: {source}"),
            Self::Codec(source) => write!(f, "codec error: {source}"),
            Self::OptimisticConcurrencyConflict {
                stream_id,
                expected,
                current_position,
            } => match current_position {
                Some(current_position) => write!(
                    f,
                    "OCC conflict for stream '{stream_id}': expected {expected:?}, current position is {current_position}"
                ),
                None => write!(
                    f,
                    "OCC conflict for stream '{stream_id}': expected {expected:?}, stream has no current position"
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
            Self::ResolveSubject(source) => Some(source),
            Self::ReadStream(source) | Self::AppendStream(source) => Some(source),
            Self::Snapshot(source) => Some(source),
            Self::Codec(source) => Some(source),
            Self::OptimisticConcurrencyConflict { .. } => None,
        }
    }
}

impl<Error> From<serde_json::Error> for JetStreamStoreError<Error>
where
    Error: From<serde_json::Error>,
{
    fn from(value: serde_json::Error) -> Self {
        Self::Codec(value.into())
    }
}

#[derive(Clone)]
pub struct JetStreamStore<Resolver> {
    js: jetstream::Context,
    events_stream: jetstream::stream::Stream,
    snapshot_bucket: kv::Store,
    snapshot_config: NatsSnapshotConfig,
    subject_resolver: Resolver,
}

impl<Resolver> JetStreamStore<Resolver> {
    pub fn new(
        js: jetstream::Context,
        events_stream: jetstream::stream::Stream,
        snapshot_bucket: kv::Store,
        snapshot_config: NatsSnapshotConfig,
        subject_resolver: Resolver,
    ) -> Self {
        Self {
            js,
            events_stream,
            snapshot_bucket,
            snapshot_config,
            subject_resolver,
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

    pub fn snapshot_config(&self) -> &NatsSnapshotConfig {
        &self.snapshot_config
    }
}

impl<StreamId, Resolver> StreamRead<StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_stream(&self, request: ReadStreamRequest<'_, StreamId>) -> Result<ReadStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        let current_position = subject_state.current_position;
        let from_sequence = stream_read_from_to_sequence(request.from);
        let events = read_subject_stream(self.events_stream(), &subject_state.subject, from_sequence)
            .await
            .map_err(JetStreamStoreError::ReadStream)?
            .into_iter()
            .map(|mut event| {
                event.stream_id = stream_id.as_ref().to_string();
                event
            })
            .collect();

        Ok(ReadStreamResponse {
            current_position,
            events,
        })
    }
}

fn stream_read_from_to_sequence(from: ReadFrom) -> u64 {
    match from {
        ReadFrom::Beginning => 1,
        ReadFrom::Position(position) => position.as_u64(),
    }
}

impl<StreamId, Resolver> StreamAppend<StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn append_stream(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let expected_state = request.stream_write_precondition;
        let events = request.events;
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        let current_position = subject_state.current_position;
        let expected_last_subject_sequence =
            resolve_expected_last_subject_sequence(stream_id, expected_state, current_position)?;

        let stream_position = append_subject_stream(
            self.as_jetstream(),
            subject_state.subject,
            expected_last_subject_sequence,
            &events,
        )
        .await
        .map_err(|source| match source {
            StreamStoreError::WrongExpectedVersion => JetStreamStoreError::OptimisticConcurrencyConflict {
                stream_id: stream_id.to_string(),
                expected: expected_state,
                current_position,
            },
            other => JetStreamStoreError::AppendStream(other),
        })?;

        Ok(AppendStreamResponse { stream_position })
    }
}

impl<StreamId, Payload, Resolver> SnapshotRead<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + SnapshotType + Send,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        crate::nats::snapshot_store::read_snapshot(self.snapshot_bucket(), request.stream_id.as_ref())
            .await
            .map(|snapshot| ReadSnapshotResponse { snapshot })
            .map_err(JetStreamStoreError::Snapshot)
    }
}

impl<StreamId, Payload, Resolver> SnapshotWrite<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + SnapshotType + Send,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, StreamId>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        crate::nats::snapshot_store::write_snapshot(
            self.snapshot_bucket(),
            request.stream_id.as_ref(),
            request.snapshot,
        )
        .await
        .map(|()| WriteSnapshotResponse)
        .map_err(JetStreamStoreError::Snapshot)
    }
}

fn resolve_expected_last_subject_sequence<StreamId, Error>(
    stream_id: &StreamId,
    expected_state: StreamWritePrecondition,
    current_position: Option<StreamPosition>,
) -> Result<Option<u64>, JetStreamStoreError<Error>>
where
    StreamId: ToString + ?Sized,
{
    match expected_state {
        StreamWritePrecondition::Any => Ok(None),
        StreamWritePrecondition::StreamExists => {
            current_position
                .map(|_| None)
                .ok_or_else(|| JetStreamStoreError::OptimisticConcurrencyConflict {
                    stream_id: stream_id.to_string(),
                    expected: StreamWritePrecondition::StreamExists,
                    current_position,
                })
        }
        StreamWritePrecondition::NoStream => Ok(Some(0)),
        StreamWritePrecondition::At(position) => Ok(Some(position.as_u64())),
    }
}

pub async fn subject_current_position(
    stream: &jetstream::stream::Stream,
    subject: &str,
) -> Result<Option<StreamPosition>, StreamStoreError> {
    match stream.get_last_raw_message_by_subject(subject).await {
        Ok(message) => StreamPosition::try_new(message.sequence)
            .map(Some)
            .map_err(|source| StreamStoreError::read_source("failed to read latest subject position", source)),
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
