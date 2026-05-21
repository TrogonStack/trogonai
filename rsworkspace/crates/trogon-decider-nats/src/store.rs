use std::{convert::Infallible, fmt};

use async_nats::jetstream::{self, kv};
#[cfg(not(coverage))]
use trogon_decider_runtime::snapshot::{
    ReadSnapshotRequest, ReadSnapshotResponse, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotType,
    WriteSnapshotRequest, WriteSnapshotResponse,
};
#[cfg(not(coverage))]
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadStreamRequest, ReadStreamResponse, SnapshotRead, SnapshotWrite,
    StreamAppend, StreamRead,
};
use trogon_decider_runtime::{ReadFrom, StreamPosition, StreamWritePrecondition};

use crate::snapshot_store::{NatsSnapshotConfig, SnapshotStoreError};
use crate::stream_store::StreamStoreError;
#[cfg(not(coverage))]
use crate::stream_store::{StreamSubjectResolver, append_stream as append_subject_stream, read_subject_stream};

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
pub enum JetStreamStoreError<Error, SnapshotPayloadError = Infallible> {
    /// Subject resolution failed before JetStream storage was accessed.
    ResolveSubject(Error),
    /// Reading stream events from JetStream failed.
    ReadStream(StreamStoreError),
    /// Appending stream events to JetStream failed.
    AppendStream(StreamStoreError),
    /// Reading or writing snapshots failed.
    Snapshot(SnapshotStoreError<SnapshotPayloadError>),
    /// Encoding or decoding a runtime payload failed.
    Codec(Error),
    /// The write precondition did not match the current stream state.
    OptimisticConcurrencyConflict(OptimisticConcurrencyConflictError),
}

impl<Error, SnapshotPayloadError> fmt::Display for JetStreamStoreError<Error, SnapshotPayloadError>
where
    Error: std::error::Error + Send + Sync + 'static,
    SnapshotPayloadError: std::fmt::Display,
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

impl<Error, SnapshotPayloadError> std::error::Error for JetStreamStoreError<Error, SnapshotPayloadError>
where
    Error: std::error::Error + Send + Sync + 'static,
    SnapshotPayloadError: std::error::Error + 'static,
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

impl JetStreamStore<()> {
    /// Starts building a store from existing JetStream stream and Key/Value handles.
    pub fn builder(
        js: jetstream::Context,
        events_stream: jetstream::stream::Stream,
        snapshot_bucket: kv::Store,
    ) -> JetStreamStoreBuilder {
        JetStreamStoreBuilder {
            js,
            events_stream,
            snapshot_bucket,
            snapshot_config: NatsSnapshotConfig::default(),
        }
    }
}

#[derive(Clone)]
/// Builder for [`JetStreamStore`].
pub struct JetStreamStoreBuilder {
    js: jetstream::Context,
    events_stream: jetstream::stream::Stream,
    snapshot_bucket: kv::Store,
    snapshot_config: NatsSnapshotConfig,
}

impl JetStreamStoreBuilder {
    /// Configures snapshot checkpoint behavior.
    pub fn with_snapshot_config(mut self, snapshot_config: NatsSnapshotConfig) -> Self {
        self.snapshot_config = snapshot_config;
        self
    }

    /// Completes the store with the application-owned subject resolver.
    pub fn with_subject_resolver<Resolver>(self, subject_resolver: Resolver) -> JetStreamStore<Resolver> {
        JetStreamStore {
            js: self.js,
            events_stream: self.events_stream,
            snapshot_bucket: self.snapshot_bucket,
            snapshot_config: self.snapshot_config,
            subject_resolver,
        }
    }
}

impl<Resolver> JetStreamStore<Resolver> {
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

    /// Returns the resolver used to map stream ids to JetStream subjects.
    pub fn subject_resolver(&self) -> &Resolver {
        &self.subject_resolver
    }
}

#[cfg(not(coverage))]
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

#[cfg(not(coverage))]
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

#[cfg(not(coverage))]
impl<StreamId, Payload, Resolver> SnapshotRead<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: SnapshotPayloadDecode + SnapshotType + Send,
    Payload::Error: std::error::Error + Send + Sync + 'static,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error, Payload::Error>;

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

#[cfg(not(coverage))]
impl<StreamId, Payload, Resolver> SnapshotWrite<Payload, StreamId> for JetStreamStore<Resolver>
where
    StreamId: AsRef<str> + Send + Sync + ?Sized,
    Payload: SnapshotPayloadEncode + SnapshotType + Send,
    Payload::Error: std::error::Error + Send + Sync + 'static,
    Resolver: StreamSubjectResolver<StreamId>,
{
    type Error = JetStreamStoreError<Resolver::Error, Payload::Error>;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).expect("test position must be non-zero")
    }

    #[test]
    fn builder_entrypoint_type_checks() {
        let _builder: fn(jetstream::Context, jetstream::stream::Stream, kv::Store) -> JetStreamStoreBuilder =
            JetStreamStore::builder;
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
}
