//! NATS JetStream storage adapter for `trogon-decider-runtime`.
//!
//! The crate maps decider stream reads, appends, and snapshots onto JetStream
//! streams and Key/Value buckets. Callers provide a [`StreamSubjectResolver`]
//! so domain stream identifiers can keep their own subject layout while the
//! adapter handles stream positions, optimistic concurrency, event envelopes,
//! and snapshot persistence.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

/// Snapshot storage helpers backed by JetStream Key/Value.
pub mod snapshot_store;
/// Event stream storage helpers backed by JetStream streams.
pub mod stream_store;

use std::fmt;

use async_nats::{
    Subject, SubjectError,
    jetstream::{self, kv},
    subject::ToSubject,
};
use trogon_nats::jetstream::JetStreamLastRawMessageBySubject;

use crate::stream_store::{append_stream as append_subject_stream, read_subject_stream};
pub use snapshot_store::{
    NatsSnapshotConfig, SnapshotChange, SnapshotStoreError, checkpoint_key, list_snapshots, maybe_advance_checkpoint,
    persist_snapshot_change, read_checkpoint, read_snapshot, read_snapshot_map, snapshot_key, write_checkpoint,
    write_snapshot,
};
pub use stream_store::{
    StreamStoreError, TROGON_EVENT_HEADER_PREFIX, TROGON_EVENT_TYPE, append_stream, read_stream, read_stream_range,
    record_stream_message,
};
use trogon_decider_runtime::snapshot::{
    ReadSnapshotRequest, ReadSnapshotResponse, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
    WriteSnapshotRequest, WriteSnapshotResponse,
};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadFrom, ReadStreamRequest, ReadStreamResponse, SnapshotRead,
    SnapshotWrite, StreamAppend, StreamPosition, StreamRead, StreamWritePrecondition,
};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Validated JetStream subject used by the decider storage adapter.
pub struct StreamSubject(Subject);

impl StreamSubject {
    /// Creates a subject after applying `async-nats` subject validation.
    pub fn new(subject: impl AsRef<str>) -> Result<Self, SubjectError> {
        Subject::validated(subject).map(Self)
    }

    /// Returns the validated subject as a string slice.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for StreamSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ToSubject for StreamSubject {
    fn to_subject(&self) -> Subject {
        self.0.clone()
    }
}

impl std::str::FromStr for StreamSubject {
    type Err = SubjectError;

    fn from_str(subject: &str) -> Result<Self, Self::Err> {
        Self::new(subject)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Resolved JetStream subject state for a decider stream.
pub struct SubjectState {
    /// JetStream subject used to store events for the requested stream.
    pub subject: StreamSubject,
    /// Latest recorded position for the subject, if the stream already exists.
    pub current_position: Option<StreamPosition>,
}

/// Resolves a domain stream identifier to the JetStream subject that stores it.
///
/// Implementations usually compose a tenant or aggregate prefix with the
/// caller's stream id, then use [`subject_current_position`] to fetch the
/// subject's latest sequence. Keeping this resolver outside [`JetStreamStore`]
/// lets applications own their subject naming scheme.
pub trait StreamSubjectResolver<StreamId: ?Sized>: Send + Sync + Clone + 'static {
    /// Error raised while resolving a stream subject or reading its state.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Resolves the stream id into a subject and current position.
    fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &StreamId,
    ) -> impl std::future::Future<Output = Result<SubjectState, Self::Error>> + Send;
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Optimistic concurrency conflict details for a failed stream append.
pub struct OptimisticConcurrencyConflictError {
    /// Domain stream id that was being appended.
    pub stream_id: String,
    /// Expected stream state supplied by the caller.
    pub expected: StreamWritePrecondition,
    /// Current stream position observed before publishing.
    pub current_position: Option<StreamPosition>,
}

impl fmt::Display for OptimisticConcurrencyConflictError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.current_position {
            Some(current_position) => write!(
                f,
                "OCC conflict for stream '{}': expected {:?}, current position is {current_position}",
                self.stream_id, self.expected
            ),
            None => write!(
                f,
                "OCC conflict for stream '{}': expected {:?}, stream has no current position",
                self.stream_id, self.expected
            ),
        }
    }
}

impl std::error::Error for OptimisticConcurrencyConflictError {}

#[derive(Debug)]
/// Error raised by [`JetStreamStore`] read, append, and snapshot operations.
pub enum JetStreamStoreError<Error> {
    /// Subject resolution failed before JetStream storage was accessed.
    ResolveSubject(Error),
    /// Reading stream events from JetStream failed.
    ReadStream(StreamStoreError),
    /// Appending stream events to JetStream failed.
    AppendStream(StreamStoreError),
    /// Reading or writing snapshots failed.
    Snapshot(SnapshotStoreError),
    /// Encoding or decoding a runtime payload failed.
    Codec(Error),
    /// The write precondition did not match the current stream state.
    OptimisticConcurrencyConflict(OptimisticConcurrencyConflictError),
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
            Self::OptimisticConcurrencyConflict(source) => source.fmt(f),
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
            Self::OptimisticConcurrencyConflict(source) => Some(source),
        }
    }
}

#[derive(Clone)]
/// JetStream-backed implementation of decider stream and snapshot traits.
///
/// The store is intentionally parameterized by a subject resolver so the
/// adapter does not prescribe a global subject topology. Event payload encoding
/// remains owned by `trogon-decider-runtime`; this crate persists the encoded
/// envelope and uses JetStream subject sequence guards for optimistic
/// concurrency.
pub struct JetStreamStore<Resolver> {
    js: jetstream::Context,
    events_stream: jetstream::stream::Stream,
    snapshot_bucket: kv::Store,
    snapshot_config: NatsSnapshotConfig,
    subject_resolver: Resolver,
}

impl<Resolver> JetStreamStore<Resolver> {
    /// Creates a store from existing JetStream stream and Key/Value handles.
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

    /// Returns the underlying JetStream context used for event publishes.
    pub fn as_jetstream(&self) -> &jetstream::Context {
        &self.js
    }

    /// Returns the JetStream stream used for event storage.
    pub fn events_stream(&self) -> &jetstream::stream::Stream {
        &self.events_stream
    }

    /// Returns the Key/Value bucket used for snapshot storage.
    pub fn snapshot_bucket(&self) -> &kv::Store {
        &self.snapshot_bucket
    }

    /// Returns the snapshot checkpoint configuration.
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
        let Some(current_position) = subject_state.current_position else {
            return Ok(ReadStreamResponse {
                current_position: None,
                events: Vec::new(),
            });
        };
        let from_sequence = stream_read_from_to_sequence(request.from);
        let to_sequence = current_position.as_u64();
        let events = read_subject_stream(
            self.events_stream(),
            stream_id.as_ref(),
            subject_state.subject.as_str(),
            from_sequence,
            to_sequence,
        )
        .await
        .map_err(JetStreamStoreError::ReadStream)?;

        Ok(ReadStreamResponse {
            current_position: Some(current_position),
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
            StreamStoreError::WrongExpectedVersion => {
                JetStreamStoreError::OptimisticConcurrencyConflict(OptimisticConcurrencyConflictError {
                    stream_id: stream_id.to_string(),
                    expected: expected_state,
                    current_position,
                })
            }
            other => JetStreamStoreError::AppendStream(other),
        })?;

        Ok(AppendStreamResponse { stream_position })
    }
}

impl<StreamId, Payload, Resolver> SnapshotRead<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: SnapshotPayloadDecode + SnapshotType + Send,
    Payload::Error: std::error::Error + Send + Sync + 'static,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, StreamId>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        crate::snapshot_store::read_snapshot(self.snapshot_bucket(), request.stream_id.as_ref())
            .await
            .map(|snapshot| ReadSnapshotResponse { snapshot })
            .map_err(JetStreamStoreError::Snapshot)
    }
}

impl<StreamId, Payload, Resolver> SnapshotWrite<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: SnapshotPayloadEncode + SnapshotType + Send,
    Payload::Error: std::error::Error + Send + Sync + 'static,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error>;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, StreamId>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        crate::snapshot_store::write_snapshot(self.snapshot_bucket(), request.stream_id.as_ref(), request.snapshot)
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
        StreamWritePrecondition::StreamExists => current_position.map(|_| None).ok_or_else(|| {
            JetStreamStoreError::OptimisticConcurrencyConflict(OptimisticConcurrencyConflictError {
                stream_id: stream_id.to_string(),
                expected: StreamWritePrecondition::StreamExists,
                current_position,
            })
        }),
        StreamWritePrecondition::NoStream => Ok(Some(0)),
        StreamWritePrecondition::At(position) => Ok(Some(position.as_u64())),
    }
}

/// Reads the latest stream position for a JetStream subject.
///
/// Missing subjects return `Ok(None)`, which lets resolvers distinguish a new
/// stream from a stream whose current position can be used for optimistic
/// concurrency.
pub async fn subject_current_position<S>(
    stream: &S,
    subject: &StreamSubject,
) -> Result<Option<StreamPosition>, StreamStoreError>
where
    S: JetStreamLastRawMessageBySubject,
{
    match stream.get_last_raw_message_by_subject(subject.as_str()).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test position must be non-zero")
    }

    #[test]
    fn stream_subject_accepts_valid_subjects() {
        let subject = StreamSubject::new("cron.jobs.events.backup").unwrap();

        assert_eq!(subject.as_str(), "cron.jobs.events.backup");
        assert_eq!(subject.to_string(), "cron.jobs.events.backup");
    }

    #[test]
    fn stream_subject_rejects_malformed_subjects() {
        for subject in ["", ".events", "events.", "events..backup", "events backup"] {
            assert!(StreamSubject::new(subject).is_err(), "{subject}");
        }
    }

    #[test]
    fn stream_read_from_maps_beginning_to_first_sequence() {
        assert_eq!(stream_read_from_to_sequence(ReadFrom::Beginning), 1);
    }

    #[test]
    fn stream_read_from_maps_position_to_sequence() {
        assert_eq!(stream_read_from_to_sequence(ReadFrom::Position(position(42))), 42);
    }

    #[test]
    fn expected_subject_sequence_allows_any_position_without_guard() {
        assert_eq!(
            resolve_expected_last_subject_sequence::<str, std::io::Error>(
                "jobs.backup",
                StreamWritePrecondition::Any,
                Some(position(7)),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn expected_subject_sequence_requires_existing_stream_before_publish() {
        assert_eq!(
            resolve_expected_last_subject_sequence::<str, std::io::Error>(
                "jobs.backup",
                StreamWritePrecondition::StreamExists,
                Some(position(7)),
            )
            .unwrap(),
            None
        );

        let error = resolve_expected_last_subject_sequence::<str, std::io::Error>(
            "jobs.backup",
            StreamWritePrecondition::StreamExists,
            None,
        )
        .unwrap_err();

        assert!(matches!(
            error,
            JetStreamStoreError::OptimisticConcurrencyConflict(conflict)
                if conflict.stream_id == "jobs.backup"
                    && conflict.expected == StreamWritePrecondition::StreamExists
                    && conflict.current_position.is_none()
        ));
    }

    #[test]
    fn expected_subject_sequence_uses_nats_no_stream_guard() {
        assert_eq!(
            resolve_expected_last_subject_sequence::<str, std::io::Error>(
                "jobs.backup",
                StreamWritePrecondition::NoStream,
                None,
            )
            .unwrap(),
            Some(0)
        );
    }

    #[test]
    fn expected_subject_sequence_uses_exact_subject_position() {
        assert_eq!(
            resolve_expected_last_subject_sequence::<str, std::io::Error>(
                "jobs.backup",
                StreamWritePrecondition::At(position(9)),
                Some(position(12)),
            )
            .unwrap(),
            Some(9)
        );
    }

    #[tokio::test]
    async fn subject_current_position_returns_none_when_no_message_found() {
        let factory = trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory::new();
        let stream = trogon_nats::jetstream::JetStreamGetStream::get_stream(&factory, "TEST_STREAM")
            .await
            .unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let current = subject_current_position(&stream, &subject).await.unwrap();

        assert_eq!(current, None);
    }

    #[tokio::test]
    async fn subject_current_position_returns_latest_sequence() {
        let factory = trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message(async_nats::jetstream::message::StreamMessage {
            subject: "test.events.backup".into(),
            sequence: 17,
            headers: async_nats::HeaderMap::new(),
            payload: bytes::Bytes::from_static(b"{}"),
            time: time::OffsetDateTime::UNIX_EPOCH,
        });
        let stream = trogon_nats::jetstream::JetStreamGetStream::get_stream(&factory, "TEST_STREAM")
            .await
            .unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let current = subject_current_position(&stream, &subject).await.unwrap();

        assert_eq!(current, Some(position(17)));
    }

    #[tokio::test]
    async fn subject_current_position_propagates_non_no_message_errors() {
        use async_nats::jetstream::stream::{LastRawMessageError, LastRawMessageErrorKind};
        let factory = trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory::new();
        factory.add_last_raw_message_error(LastRawMessageError::new(LastRawMessageErrorKind::Other));
        let stream = trogon_nats::jetstream::JetStreamGetStream::get_stream(&factory, "TEST_STREAM")
            .await
            .unwrap();

        let subject = StreamSubject::new("test.events.backup").unwrap();
        let error = subject_current_position(&stream, &subject).await.unwrap_err();

        assert!(matches!(error, StreamStoreError::Read { .. }));
    }
}
