#![cfg_attr(coverage, allow(dead_code, unused_imports))]

use async_nats::jetstream::{self, kv};
use trogon_decider_nats::{
    JetStreamStore, NatsSnapshotConfig, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite,
    StreamAppend, StreamRead, WriteSnapshotRequest, WriteSnapshotResponse,
};

use crate::config::ScheduleWriteState;
use crate::error::SchedulerError;
use crate::nats::{event_subject, resolve_event_subject_state};
#[cfg(not(coverage))]
use crate::projections::project_appended_events;

// The real `JetStreamStore` operations (and the `JetStreamLastRawMessageBySubject`
// impl that `subject_current_position` requires) are compiled out under coverage,
// so the method bodies that reach NATS are stubbed there. The store can only be
// constructed by `connect_store`, which is itself a coverage stub.
#[cfg(coverage)]
fn coverage_unavailable(context: &'static str) -> SchedulerError {
    SchedulerError::event_source(context, std::io::Error::other("coverage"))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ScheduleEventSubjectResolver;

impl StreamSubjectResolver<str> for ScheduleEventSubjectResolver {
    type Error = SchedulerError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
        #[cfg(not(coverage))]
        {
            let subject = StreamSubject::new(event_subject(stream_id))
                .map_err(|source| SchedulerError::event_source("failed to resolve schedule event subject", source))?;
            let current_position = subject_current_position(events_stream, &subject)
                .await
                .map_err(|source| SchedulerError::event_source("failed to read latest stream position", source))?;
            let canonical_state = current_position.map(|position| ScheduleWriteState::new(Some(position), true));
            let state = resolve_event_subject_state(canonical_state);

            Ok(SubjectState {
                subject,
                current_position: state.write_state.current_position(),
            })
        }
        #[cfg(coverage)]
        {
            let _ = (events_stream, stream_id);
            Err(coverage_unavailable(
                "coverage stub does not resolve schedule event subject state",
            ))
        }
    }
}

#[derive(Clone)]
pub struct EventStore {
    inner: JetStreamStore<ScheduleEventSubjectResolver>,
    schedules_bucket: kv::Store,
}

impl EventStore {
    pub fn new(
        js: jetstream::Context,
        events_stream: jetstream::stream::Stream,
        command_snapshot_bucket: kv::Store,
        schedules_bucket: kv::Store,
    ) -> Self {
        Self {
            inner: JetStreamStore::builder(js, events_stream, command_snapshot_bucket)
                .with_snapshot_config(NatsSnapshotConfig::without_checkpoint())
                .with_subject_resolver(ScheduleEventSubjectResolver),
            schedules_bucket,
        }
    }

    pub fn events_stream(&self) -> &jetstream::stream::Stream {
        self.inner.events_stream()
    }
}

impl StreamRead<str> for EventStore {
    type Error = SchedulerError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        #[cfg(not(coverage))]
        {
            self.inner.read_stream(request).await.map_err(SchedulerError::from)
        }
        #[cfg(coverage)]
        {
            let _ = request;
            Err(coverage_unavailable("coverage stub does not read schedule streams"))
        }
    }
}

impl StreamAppend<str> for EventStore {
    type Error = SchedulerError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        #[cfg(not(coverage))]
        {
            let stream_id = request.stream_id;
            let projected_events = request.events.clone();
            let outcome = self.inner.append_stream(request).await.map_err(SchedulerError::from)?;

            // The append is the source of truth and has committed. The KV read model
            // is a derived projection, so a projection failure must NOT turn a durable
            // append into a caller-visible error: the checkpoint is left unadvanced and
            // catch-up rebuilds the affected schedule on the next start (the rebuild is
            // idempotent). Surface the failure loudly for observability instead.
            if let Err(source) = project_appended_events(
                &self.schedules_bucket,
                stream_id,
                projected_events.as_slice(),
                outcome.stream_position,
            )
            .await
            {
                tracing::error!(
                    schedule_id = %stream_id,
                    stream_position = outcome.stream_position.as_u64(),
                    %source,
                    "failed to project appended schedule events into the read model; \
                     the append is durable and catch-up will repair the read model on restart"
                );
            }

            Ok(outcome)
        }
        #[cfg(coverage)]
        {
            let _ = request;
            Err(coverage_unavailable("coverage stub does not append schedule streams"))
        }
    }
}

impl<Payload> SnapshotRead<Payload, str> for EventStore
where
    Payload: SnapshotPayloadDecode + SnapshotType + Send,
    <Payload as SnapshotPayloadDecode>::Error: std::error::Error + Send + Sync + 'static,
    <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = SchedulerError;

    async fn read_snapshot(
        &self,
        request: ReadSnapshotRequest<'_, str>,
    ) -> Result<ReadSnapshotResponse<Payload>, Self::Error> {
        #[cfg(not(coverage))]
        {
            self.inner.read_snapshot(request).await.map_err(SchedulerError::from)
        }
        #[cfg(coverage)]
        {
            let _ = request;
            Err(coverage_unavailable("coverage stub does not read schedule snapshots"))
        }
    }
}

impl<Payload> SnapshotWrite<Payload, str> for EventStore
where
    Payload: SnapshotPayloadEncode + SnapshotType + Send,
    <Payload as SnapshotPayloadEncode>::Error: std::error::Error + Send + Sync + 'static,
    <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = SchedulerError;

    async fn write_snapshot(
        &self,
        request: WriteSnapshotRequest<'_, Payload, str>,
    ) -> Result<WriteSnapshotResponse, Self::Error> {
        #[cfg(not(coverage))]
        {
            self.inner.write_snapshot(request).await.map_err(SchedulerError::from)
        }
        #[cfg(coverage)]
        {
            let _ = request;
            Err(coverage_unavailable("coverage stub does not write schedule snapshots"))
        }
    }
}
