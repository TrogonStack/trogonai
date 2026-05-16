mod snapshot;
mod stream;

use std::fmt;

use async_nats::jetstream::{self, kv};

use crate::nats::snapshot_store::SnapshotStoreError;
use crate::nats::stream_store::StreamStoreError;
use crate::{StreamPosition, StreamWritePrecondition};

pub use stream::subject_current_position;

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
}
