use async_nats::jetstream::{self, kv};
use trogon_decider_nats::{
    JetStreamStore, NatsSnapshotConfig, StreamSubject, StreamSubjectResolver, SubjectState, subject_current_position,
};
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest,
    ReadStreamResponse, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite,
    StreamAppend, StreamRead, WriteSnapshotRequest, WriteSnapshotResponse,
};

use crate::{
    config::ScheduleWriteState,
    error::SchedulerError,
    nats::{event_subject, resolve_event_subject_state},
    projections::project_appended_events,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ScheduleEventSubjectResolver;

impl StreamSubjectResolver<str> for ScheduleEventSubjectResolver {
    type Error = SchedulerError;

    async fn resolve_subject_state(
        &self,
        events_stream: &jetstream::stream::Stream,
        stream_id: &str,
    ) -> Result<SubjectState, Self::Error> {
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
        self.inner.read_stream(request).await.map_err(SchedulerError::from)
    }
}

impl StreamAppend<str> for EventStore {
    type Error = SchedulerError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let projected_events = request.events.clone();
        let outcome = self.inner.append_stream(request).await.map_err(SchedulerError::from)?;

        project_appended_events(
            &self.schedules_bucket,
            stream_id,
            projected_events.as_slice(),
            outcome.stream_position,
        )
        .await?;

        Ok(outcome)
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
        self.inner.read_snapshot(request).await.map_err(SchedulerError::from)
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
        self.inner.write_snapshot(request).await.map_err(SchedulerError::from)
    }
}
