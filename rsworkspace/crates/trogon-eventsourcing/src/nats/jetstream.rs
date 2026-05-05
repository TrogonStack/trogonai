use std::fmt;

use async_nats::jetstream::{self, kv};
use serde::{Serialize, de::DeserializeOwned};

use crate::nats::snapshot_store::SnapshotStoreError;
use crate::nats::streams::{StreamStoreError, append_stream as append_subject_stream, read_subject_stream};
use crate::snapshot::{ReadSnapshotRequest, ReadSnapshotResponse, WriteSnapshotRequest, WriteSnapshotResponse};
use crate::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, SnapshotRead, SnapshotWrite,
    StreamAppend, StreamRead, StreamState,
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

#[derive(Debug)]
pub enum JetStreamStoreError<Error> {
    ResolveSubject(Error),
    ReadStream(StreamStoreError),
    AppendStream(StreamStoreError),
    Snapshot(SnapshotStoreError),
    Codec(Error),
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
    subject_resolver: Resolver,
}

impl<Resolver> JetStreamStore<Resolver> {
    pub fn new(
        js: jetstream::Context,
        events_stream: jetstream::stream::Stream,
        snapshot_bucket: kv::Store,
        subject_resolver: Resolver,
    ) -> Self {
        Self {
            js,
            events_stream,
            snapshot_bucket,
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

    pub async fn read_stream<StreamId>(
        &self,
        request: ReadStreamRequest<'_, StreamId>,
    ) -> Result<ReadStreamResponse, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
        Resolver: StreamSubjectResolver<StreamId>,
    {
        let stream_id = request.stream_id;
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        let current_version = subject_state.current_version;
        let events = read_subject_stream(self.events_stream(), &subject_state.subject, request.from_sequence)
            .await
            .map_err(JetStreamStoreError::ReadStream)?
            .into_iter()
            .map(|mut event| {
                event.event_stream_id = stream_id.as_ref().to_string();
                event
            })
            .collect();

        Ok(ReadStreamResponse {
            current_version,
            events,
        })
    }

    pub async fn append_stream<StreamId>(
        &self,
        request: AppendStreamRequest<'_, StreamId>,
    ) -> Result<AppendStreamResponse, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
        Resolver: StreamSubjectResolver<StreamId>,
    {
        let stream_id = request.stream_id;
        let expected_state = request.stream_state;
        let events = request.events;
        let subject_state = self
            .subject_resolver
            .resolve_subject_state(self.events_stream(), stream_id)
            .await
            .map_err(JetStreamStoreError::ResolveSubject)?;
        if events.iter().any(|event| event.stream_id() != stream_id.as_ref()) {
            return Err(JetStreamStoreError::AppendStream(StreamStoreError::publish_source(
                "failed to publish stream event batch",
                std::io::Error::other(format!("batch contains events outside stream '{}'", stream_id.as_ref())),
            )));
        }
        let current_version = subject_state.current_version;
        let expected_last_subject_sequence =
            resolve_expected_last_subject_sequence(stream_id, expected_state, current_version)?;

        let next_expected_version = append_subject_stream(
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
                current_version,
            },
            other => JetStreamStoreError::AppendStream(other),
        })?;

        Ok(AppendStreamResponse { next_expected_version })
    }

    pub async fn read_snapshot<StreamId, Payload>(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> Result<ReadSnapshotResponse<Payload>, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + Send + Sync + ?Sized,
        Payload: Serialize + DeserializeOwned + Send,
        Resolver: StreamSubjectResolver<StreamId>,
    {
        crate::nats::snapshot_store::read_snapshot(self.snapshot_bucket(), &request.config, request.stream_id.as_ref())
            .await
            .map(ReadSnapshotResponse::new)
            .map_err(JetStreamStoreError::Snapshot)
    }

    pub async fn write_snapshot<StreamId, Payload>(
        &self,
        request: WriteSnapshotRequest<'_, Payload, StreamId>,
    ) -> Result<WriteSnapshotResponse, JetStreamStoreError<Resolver::Error>>
    where
        StreamId: AsRef<str> + Send + Sync + ?Sized,
        Payload: Serialize + DeserializeOwned + Send,
        Resolver: StreamSubjectResolver<StreamId>,
    {
        crate::nats::snapshot_store::write_snapshot(
            self.snapshot_bucket(),
            &request.config,
            request.stream_id.as_ref(),
            request.snapshot,
        )
        .await
        .map(|()| WriteSnapshotResponse::new())
        .map_err(JetStreamStoreError::Snapshot)
    }
}

fn resolve_expected_last_subject_sequence<StreamId, Error>(
    stream_id: &StreamId,
    expected_state: StreamState,
    current_version: Option<u64>,
) -> Result<Option<u64>, JetStreamStoreError<Error>>
where
    StreamId: ToString + ?Sized,
{
    match expected_state {
        StreamState::Any => Ok(None),
        StreamState::StreamExists => {
            current_version
                .map(|_| None)
                .ok_or_else(|| JetStreamStoreError::OptimisticConcurrencyConflict {
                    stream_id: stream_id.to_string(),
                    expected: StreamState::StreamExists,
                    current_version,
                })
        }
        StreamState::NoStream => Ok(Some(0)),
        StreamState::StreamRevision(version) => Ok(Some(version)),
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

impl<StreamId, Resolver> StreamRead<StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + ToString + Send + Sync + ?Sized,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_stream(&self, request: ReadStreamRequest<'_, StreamId>) -> Result<ReadStreamResponse, Self::Error> {
        JetStreamStore::read_stream(self, request).await
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
        JetStreamStore::append_stream(self, request).await
    }
}

impl<StreamId, Payload, Resolver> SnapshotRead<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + Send,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        JetStreamStore::read_snapshot(self, request).await
    }
}

impl<StreamId, Payload, Resolver> SnapshotWrite<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: Serialize + DeserializeOwned + Send,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, StreamId>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        JetStreamStore::write_snapshot(self, request).await
    }
}
