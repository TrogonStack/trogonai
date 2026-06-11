use std::collections::BTreeMap;

use async_nats::jetstream::{
    self,
    consumer::{DeliverPolicy, ReplayPolicy, pull},
    kv,
};
use futures::StreamExt;
use trogon_decider_nats::record_stream_message;
use trogon_decider_runtime::{Event, EventData, EventDecode, StreamEvent, StreamPosition};
use trogon_nats::SubjectTokenViolation;
use trogon_nats::jetstream::{JetStreamGetKeyValue, JetStreamGetStream};

use chrono::{TimeZone, Utc};

use crate::{
    DeliveryKind, ScheduleEventCase, ScheduleKind, ScheduleStatusKind, SourceKind,
    error::SchedulerError,
    kv::{EVENTS_SUBJECT_PREFIX, SCHEDULES_CHECKPOINT_KEY, open_events_stream, open_schedules_bucket},
    read_model::{
        MessageContent, MessageEnvelope, MessageHeaders, Schedule, ScheduleEventDelivery, ScheduleEventSamplingSource,
        ScheduleEventSchedule, ScheduleEventStatus,
    },
    v1,
};

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
enum ProjectionChange {
    Upsert(Schedule),
    Delete(String),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
enum ScheduleStreamState {
    Initial,
    Present(Schedule),
    Deleted(String),
}

#[derive(Debug)]
enum ScheduleTransitionError {
    InvalidEventId { id: String, source: SubjectTokenViolation },
    MismatchedEventScheduleId { stream_id: String, schedule_id: String },
    MalformedEvent { context: &'static str },
    CannotAddExistingSchedule { id: String },
    CannotAddDeletedSchedule { id: String },
    MissingScheduleForStateChange { id: String },
    DeletedScheduleForStateChange { id: String },
    DeletedScheduleForRemoval { id: String },
}

const fn initial_state() -> ScheduleStreamState {
    ScheduleStreamState::Initial
}

fn apply(
    stream_id: &str,
    state: ScheduleStreamState,
    event: &v1::ScheduleEvent,
) -> Result<ScheduleStreamState, ScheduleTransitionError> {
    validate_event_schedule_id(stream_id).map_err(|source| ScheduleTransitionError::InvalidEventId {
        id: stream_id.to_string(),
        source,
    })?;
    validate_event_payload_schedule_id(stream_id, event)?;

    match (state, &event.event) {
        (ScheduleStreamState::Initial, Some(ScheduleEventCase::ScheduleCreated(inner))) => {
            Ok(ScheduleStreamState::Present(project_created_job(inner)?))
        }
        (ScheduleStreamState::Initial, Some(ScheduleEventCase::SchedulePaused(_))) => {
            Err(ScheduleTransitionError::MissingScheduleForStateChange {
                id: stream_id.to_string(),
            })
        }
        (ScheduleStreamState::Initial, Some(ScheduleEventCase::ScheduleResumed(_))) => {
            Err(ScheduleTransitionError::MissingScheduleForStateChange {
                id: stream_id.to_string(),
            })
        }
        (ScheduleStreamState::Initial, Some(ScheduleEventCase::ScheduleRemoved(_))) => {
            Ok(ScheduleStreamState::Deleted(stream_id.to_string()))
        }
        (ScheduleStreamState::Present(job), Some(ScheduleEventCase::ScheduleCreated(_))) => {
            Err(ScheduleTransitionError::CannotAddExistingSchedule { id: job.id })
        }
        (ScheduleStreamState::Present(mut job), Some(ScheduleEventCase::SchedulePaused(_))) => {
            job.status = ScheduleEventStatus::Paused;
            Ok(ScheduleStreamState::Present(job))
        }
        (ScheduleStreamState::Present(mut job), Some(ScheduleEventCase::ScheduleResumed(_))) => {
            job.status = ScheduleEventStatus::Scheduled;
            Ok(ScheduleStreamState::Present(job))
        }
        (ScheduleStreamState::Present(job), Some(ScheduleEventCase::ScheduleRemoved(_))) => {
            Ok(ScheduleStreamState::Deleted(job.id))
        }
        (ScheduleStreamState::Deleted(id), Some(ScheduleEventCase::ScheduleCreated(_))) => {
            Err(ScheduleTransitionError::CannotAddDeletedSchedule { id })
        }
        (ScheduleStreamState::Deleted(id), Some(ScheduleEventCase::SchedulePaused(_))) => {
            Err(ScheduleTransitionError::DeletedScheduleForStateChange { id })
        }
        (ScheduleStreamState::Deleted(id), Some(ScheduleEventCase::ScheduleResumed(_))) => {
            Err(ScheduleTransitionError::DeletedScheduleForStateChange { id })
        }
        (ScheduleStreamState::Deleted(id), Some(ScheduleEventCase::ScheduleRemoved(_))) => {
            Err(ScheduleTransitionError::DeletedScheduleForRemoval { id })
        }
        (_, None) => Err(ScheduleTransitionError::MalformedEvent {
            context: "schedule event has no supported case",
        }),
    }
}

fn validate_event_payload_schedule_id(
    stream_id: &str,
    event: &v1::ScheduleEvent,
) -> Result<(), ScheduleTransitionError> {
    let Some(schedule_id) = event_schedule_id(event) else {
        return Ok(());
    };
    validate_event_schedule_id(schedule_id).map_err(|source| ScheduleTransitionError::InvalidEventId {
        id: schedule_id.to_string(),
        source,
    })?;
    if schedule_id == stream_id {
        Ok(())
    } else {
        Err(ScheduleTransitionError::MismatchedEventScheduleId {
            stream_id: stream_id.to_string(),
            schedule_id: schedule_id.to_string(),
        })
    }
}

fn event_schedule_id(event: &v1::ScheduleEvent) -> Option<&str> {
    match &event.event {
        Some(ScheduleEventCase::ScheduleCreated(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::SchedulePaused(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleResumed(inner)) => Some(&inner.schedule_id),
        Some(ScheduleEventCase::ScheduleRemoved(inner)) => Some(&inner.schedule_id),
        None => None,
    }
}

fn project_created_job(event: &v1::ScheduleCreated) -> Result<Schedule, ScheduleTransitionError> {
    let schedule = event
        .schedule
        .as_option()
        .ok_or(ScheduleTransitionError::MalformedEvent {
            context: "job details has no schedule",
        })?;
    let delivery = event
        .delivery
        .as_option()
        .ok_or(ScheduleTransitionError::MalformedEvent {
            context: "job details has no delivery",
        })?;
    let message = event
        .message
        .as_option()
        .ok_or(ScheduleTransitionError::MalformedEvent {
            context: "job details has no message",
        })?;
    Ok(Schedule {
        id: event.schedule_id.to_string(),
        status: project_status(event.status.as_option()),
        schedule: project_schedule(schedule)?,
        delivery: project_delivery(delivery)?,
        message: project_message(message),
    })
}

fn project_status(status: Option<&v1::ScheduleStatus>) -> ScheduleEventStatus {
    if matches!(
        status.and_then(|s| s.kind.as_ref()),
        Some(ScheduleStatusKind::Paused(_))
    ) {
        ScheduleEventStatus::Paused
    } else {
        ScheduleEventStatus::Scheduled
    }
}

fn timestamp_to_datetime(ts: &buffa_types::google::protobuf::Timestamp) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(ts.seconds, ts.nanos as u32)
        .single()
        .unwrap_or_default()
}

fn project_schedule(schedule: &v1::Schedule) -> Result<ScheduleEventSchedule, ScheduleTransitionError> {
    match schedule.kind.as_ref() {
        Some(ScheduleKind::At(inner)) => {
            let at = inner.at.as_option().map(timestamp_to_datetime).unwrap_or_default();
            Ok(ScheduleEventSchedule::At { at })
        }
        Some(ScheduleKind::Every(inner)) => {
            let every_sec = inner.every.as_option().map(|d| d.seconds as u64).unwrap_or(0);
            Ok(ScheduleEventSchedule::Every { every_sec })
        }
        Some(ScheduleKind::Cron(inner)) => Ok(ScheduleEventSchedule::Cron {
            expr: inner.expr.clone(),
            timezone: inner
                .timezone
                .as_option()
                .map(|tz| tz.id.clone())
                .filter(|s| !s.is_empty()),
        }),
        Some(ScheduleKind::Rrule(inner)) => Ok(ScheduleEventSchedule::RRule {
            dtstart: inner.dtstart.as_option().map(timestamp_to_datetime).unwrap_or_default(),
            rrule: inner.rrule.clone(),
            timezone: inner
                .timezone
                .as_option()
                .map(|tz| tz.id.clone())
                .filter(|s| !s.is_empty()),
            rdate: inner.rdate.iter().map(timestamp_to_datetime).collect(),
            exdate: inner.exdate.iter().map(timestamp_to_datetime).collect(),
        }),
        None => Err(ScheduleTransitionError::MalformedEvent {
            context: "job schedule has no supported case",
        }),
    }
}

fn project_delivery(delivery: &v1::Delivery) -> Result<ScheduleEventDelivery, ScheduleTransitionError> {
    match delivery.kind.as_ref() {
        Some(DeliveryKind::NatsMessage(inner)) => Ok(ScheduleEventDelivery::NatsMessage {
            subject: inner.subject.clone(),
            ttl_sec: inner.ttl.as_option().map(|d| d.seconds as u64),
            source: inner.source.as_option().map(project_sampling_source).transpose()?,
        }),
        None => Err(ScheduleTransitionError::MalformedEvent {
            context: "job delivery has no supported case",
        }),
    }
}

fn project_sampling_source(
    source: &v1::delivery::nats_message::Source,
) -> Result<ScheduleEventSamplingSource, ScheduleTransitionError> {
    match source.kind.as_ref() {
        Some(SourceKind::LatestFromSubject(inner)) => Ok(ScheduleEventSamplingSource::LatestFromSubject {
            subject: inner.subject.clone(),
        }),
        None => Err(ScheduleTransitionError::MalformedEvent {
            context: "job sampling source has no supported case",
        }),
    }
}

fn project_message(message: &v1::Message) -> MessageEnvelope {
    let content_str = message
        .content
        .as_option()
        .map(|c| String::from_utf8_lossy(&c.data).into_owned())
        .unwrap_or_default();
    MessageEnvelope {
        content: MessageContent::new(content_str),
        headers: MessageHeaders::from_pairs(
            message
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone())),
        ),
    }
}

fn projection_change(before: &ScheduleStreamState, after: &ScheduleStreamState) -> Option<ProjectionChange> {
    match (before, after) {
        (ScheduleStreamState::Initial, ScheduleStreamState::Initial) => None,
        (_, ScheduleStreamState::Present(spec)) => Some(ProjectionChange::Upsert(spec.clone())),
        (ScheduleStreamState::Present(spec), ScheduleStreamState::Initial | ScheduleStreamState::Deleted(_)) => {
            Some(ProjectionChange::Delete(spec.id.to_string()))
        }
        (ScheduleStreamState::Initial, ScheduleStreamState::Deleted(_))
        | (ScheduleStreamState::Deleted(_), ScheduleStreamState::Initial)
        | (ScheduleStreamState::Deleted(_), ScheduleStreamState::Deleted(_)) => None,
    }
}

impl From<Schedule> for ScheduleStreamState {
    fn from(job: Schedule) -> Self {
        Self::Present(job)
    }
}

impl std::fmt::Display for ScheduleTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEventId { id, .. } => write!(f, "schedule event id '{id}' is invalid"),
            Self::MismatchedEventScheduleId { stream_id, schedule_id } => {
                write!(
                    f,
                    "schedule event id '{schedule_id}' does not match stream id '{stream_id}'"
                )
            }
            Self::MalformedEvent { context } => write!(f, "schedule event is malformed: {context}"),
            Self::CannotAddExistingSchedule { id } => write!(f, "job '{id}' already exists"),
            Self::CannotAddDeletedSchedule { id } => {
                write!(f, "job '{id}' was deleted and cannot be added again")
            }
            Self::MissingScheduleForStateChange { id } => {
                write!(f, "missing job for state change '{id}'")
            }
            Self::DeletedScheduleForStateChange { id } => {
                write!(f, "deleted schedule '{id}' cannot change state")
            }
            Self::DeletedScheduleForRemoval { id } => {
                write!(f, "job '{id}' was already deleted")
            }
        }
    }
}

impl std::error::Error for ScheduleTransitionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidEventId { source, .. } => Some(source),
            Self::MismatchedEventScheduleId { .. }
            | Self::MalformedEvent { .. }
            | Self::CannotAddExistingSchedule { .. }
            | Self::CannotAddDeletedSchedule { .. }
            | Self::MissingScheduleForStateChange { .. }
            | Self::DeletedScheduleForStateChange { .. }
            | Self::DeletedScheduleForRemoval { .. } => None,
        }
    }
}

pub(crate) async fn catch_up_schedules_read_model<J>(js: &J) -> Result<(), SchedulerError>
where
    J: JetStreamGetKeyValue<Store = kv::Store> + JetStreamGetStream<Stream = jetstream::stream::Stream>,
{
    let stream: jetstream::stream::Stream = open_events_stream(js).await?;
    let info = stream.get_info().await.map_err(|source| {
        SchedulerError::event_source(
            "failed to query events stream info for schedules read-model catch-up",
            source,
        )
    })?;
    if info.state.messages == 0 {
        return Ok(());
    }

    let bucket = open_schedules_bucket(js).await?;
    let checkpoint = read_read_model_checkpoint(&bucket).await?;
    if checkpoint >= info.state.last_sequence {
        return Ok(());
    }

    let mut states = read_model_state_map(&bucket).await?;
    let start = checkpoint.max(info.state.first_sequence.saturating_sub(1)) + 1;

    let consumer = stream
        .create_consumer(event_replay_consumer_config(start))
        .await
        .map_err(|source| {
            SchedulerError::event_source("failed to create schedules read-model catch-up consumer", source)
        })?;
    let mut messages = consumer.messages().await.map_err(|source| {
        SchedulerError::event_source("failed to open schedules read-model catch-up stream", source)
    })?;

    while let Some(message) = messages.next().await {
        let message = message.map_err(|source| {
            SchedulerError::event_source(
                "failed to read schedule event during schedules read-model catch-up",
                source,
            )
        })?;
        let sequence = event_message_sequence(&message, "failed to read schedules read-model catch-up event metadata")?;
        if sequence > info.state.last_sequence {
            break;
        }
        let reached_tail = sequence >= info.state.last_sequence;
        let event = decode_recorded_delivery_message(&message)?;
        let data = event.decode::<v1::ScheduleEvent>().map_err(|source| {
            SchedulerError::event_source(
                "failed to decode schedule event during schedules read-model catch-up",
                source,
            )
        })?;
        let Some(data) = data.into_decoded() else {
            write_read_model_checkpoint(&bucket, sequence).await?;
            if reached_tail {
                break;
            }
            continue;
        };
        let stream_id = schedule_id_from_event_subject(event.stream_id())?;
        if let Some(change) = apply_event_to_read_model_state(&mut states, &stream_id, &data)? {
            apply_projection_change(&bucket, &change).await?;
        }
        write_read_model_checkpoint(&bucket, sequence).await?;
        if reached_tail {
            break;
        }
    }

    Ok(())
}

pub(crate) async fn project_appended_events(
    bucket: &kv::Store,
    job_id: &str,
    events: &[Event],
    final_position: StreamPosition,
) -> Result<(), SchedulerError> {
    if events.is_empty() {
        return Ok(());
    }
    validate_event_schedule_id(job_id).map_err(|source| {
        SchedulerError::invalid_schedule_spec(crate::ScheduleSpecError::InvalidId {
            id: job_id.to_string(),
            source,
        })
    })?;

    let mut states = BTreeMap::new();
    if let Some(job) = read_projected_schedule(bucket, job_id).await? {
        states.insert(job_id.to_string(), ScheduleStreamState::from(job));
    }

    for event in events {
        let decoded = v1::ScheduleEvent::decode(EventData::new(&event.r#type, &event.content)).map_err(|source| {
            SchedulerError::event_source("failed to decode schedule event for schedules read model", source)
        })?;
        let Some(decoded) = decoded.into_decoded() else {
            continue;
        };
        if let Some(change) = apply_event_to_read_model_state(&mut states, job_id, &decoded)? {
            apply_projection_change(bucket, &change).await?;
        }
    }
    maybe_advance_read_model_checkpoint(bucket, final_position.as_u64()).await
}

fn decode_recorded_job_event(
    message: async_nats::jetstream::message::StreamMessage,
) -> Result<StreamEvent, SchedulerError> {
    let stream_id = message.subject.to_string();
    record_stream_message(message, stream_id)
        .map_err(|source| SchedulerError::event_source("failed to decode stored schedule event", source))
}

fn decode_recorded_delivery_message(message: &async_nats::jetstream::Message) -> Result<StreamEvent, SchedulerError> {
    let stream_message =
        async_nats::jetstream::message::StreamMessage::try_from(message.message.clone()).map_err(|source| {
            SchedulerError::event_source("failed to reconstruct stream message from event delivery", source)
        })?;

    decode_recorded_job_event(stream_message)
}

fn event_replay_consumer_config(start_sequence: u64) -> pull::OrderedConfig {
    pull::OrderedConfig {
        deliver_policy: DeliverPolicy::ByStartSequence { start_sequence },
        replay_policy: ReplayPolicy::Instant,
        ..Default::default()
    }
}

fn event_message_sequence(message: &jetstream::Message, context: &'static str) -> Result<u64, SchedulerError> {
    message
        .info()
        .map(|info| info.stream_sequence)
        .map_err(|source| SchedulerError::event_source(context, std::io::Error::other(source.to_string())))
}

fn is_read_model_metadata_key(key: &str) -> bool {
    key == SCHEDULES_CHECKPOINT_KEY
}

async fn read_projected_schedule(bucket: &kv::Store, id: &str) -> Result<Option<Schedule>, SchedulerError> {
    let Some(entry) = bucket
        .entry(id.to_string())
        .await
        .map_err(|source| SchedulerError::kv_source("failed to read projected schedule", source))?
    else {
        return Ok(None);
    };

    serde_json::from_slice(&entry.value)
        .map(Some)
        .map_err(SchedulerError::from)
}

async fn read_model_state_map(bucket: &kv::Store) -> Result<BTreeMap<String, ScheduleStreamState>, SchedulerError> {
    let mut keys = bucket
        .keys()
        .await
        .map_err(|source| SchedulerError::kv_source("failed to list schedules read-model keys", source))?;
    let mut states = BTreeMap::new();

    while let Some(result) = keys.next().await {
        let key =
            result.map_err(|source| SchedulerError::kv_source("failed to read schedules read-model key", source))?;
        if is_read_model_metadata_key(&key) {
            continue;
        }
        if let Some(job) = read_projected_schedule(bucket, &key).await? {
            states.insert(key, ScheduleStreamState::Present(job));
        }
    }

    Ok(states)
}

async fn read_read_model_checkpoint(bucket: &kv::Store) -> Result<u64, SchedulerError> {
    let Some(entry) = bucket
        .entry(SCHEDULES_CHECKPOINT_KEY.to_string())
        .await
        .map_err(|source| SchedulerError::kv_source("failed to read schedules read-model checkpoint", source))?
    else {
        return Ok(0);
    };

    String::from_utf8(entry.value.to_vec())
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| {
            SchedulerError::kv_source(
                "failed to decode schedules read-model checkpoint",
                std::io::Error::other(SCHEDULES_CHECKPOINT_KEY),
            )
        })
}

async fn write_read_model_checkpoint(bucket: &kv::Store, sequence: u64) -> Result<(), SchedulerError> {
    bucket
        .put(SCHEDULES_CHECKPOINT_KEY.to_string(), sequence.to_string().into())
        .await
        .map(|_| ())
        .map_err(|source| SchedulerError::kv_source("failed to write schedules read-model checkpoint", source))
}

async fn maybe_advance_read_model_checkpoint(bucket: &kv::Store, sequence: u64) -> Result<(), SchedulerError> {
    let current = read_read_model_checkpoint(bucket).await?;
    if current != sequence.saturating_sub(1) {
        return Ok(());
    }

    write_read_model_checkpoint(bucket, sequence).await
}

async fn apply_projection_change(kv: &kv::Store, change: &ProjectionChange) -> Result<(), SchedulerError> {
    match change {
        ProjectionChange::Upsert(job) => {
            let value = serde_json::to_vec(job)?;
            kv.put(job.id.to_string(), value.into())
                .await
                .map_err(|source| SchedulerError::kv_source("failed to store projected job state", source))?;
        }
        ProjectionChange::Delete(id) => {
            kv.delete(id.clone())
                .await
                .map_err(|source| SchedulerError::kv_source("failed to delete projected job state", source))?;
        }
    }

    Ok(())
}

fn apply_event_to_read_model_state(
    states: &mut BTreeMap<String, ScheduleStreamState>,
    stream_id: &str,
    event: &v1::ScheduleEvent,
) -> Result<Option<ProjectionChange>, SchedulerError> {
    let current_state = states.get(stream_id).cloned().unwrap_or_else(initial_state);
    let next_state = apply(stream_id, current_state.clone(), event).map_err(|source| {
        SchedulerError::event_source("failed to apply schedule event to schedules read model", source)
    })?;
    let change = projection_change(&current_state, &next_state);

    match next_state.clone() {
        ScheduleStreamState::Present(_) | ScheduleStreamState::Deleted(_) => {
            states.insert(stream_id.to_string(), next_state);
        }
        ScheduleStreamState::Initial => {
            states.remove(stream_id);
        }
    }

    Ok(change)
}

fn schedule_id_from_event_subject(subject: &str) -> Result<String, SchedulerError> {
    let raw_id = subject.strip_prefix(EVENTS_SUBJECT_PREFIX).ok_or_else(|| {
        SchedulerError::event_source(
            "failed to derive schedule stream id from event subject",
            std::io::Error::other(subject.to_string()),
        )
    })?;

    validate_event_schedule_id(raw_id)
        .map(|()| raw_id.to_string())
        .map_err(|source| {
            SchedulerError::invalid_schedule_spec(crate::ScheduleSpecError::InvalidId {
                id: raw_id.to_string(),
                source,
            })
        })
}

fn validate_event_schedule_id(id: &str) -> Result<(), SubjectTokenViolation> {
    trogon_nats::NatsToken::new(id).map(|_| ())
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;
    use buffa_types::google::protobuf::{Duration, Timestamp};
    use chrono::{DateTime, Utc};

    use super::*;
    use crate::v1;
    use crate::{
        MessageContent, MessageEnvelope, MessageHeaders, Schedule, ScheduleEventDelivery, ScheduleEventSchedule,
        ScheduleEventStatus,
    };

    fn timestamp_from_str(rfc3339: &str) -> Timestamp {
        let dt = DateTime::parse_from_rfc3339(rfc3339).unwrap();
        Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
            ..Default::default()
        }
    }

    fn expected_schedule(id: &str) -> Schedule {
        Schedule {
            id: id.to_string(),
            status: ScheduleEventStatus::Scheduled,
            schedule: ScheduleEventSchedule::Every { every_sec: 30 },
            delivery: ScheduleEventDelivery::NatsMessage {
                subject: "agent.run".to_string(),
                ttl_sec: None,
                source: None,
            },
            message: MessageEnvelope {
                content: MessageContent::from_static(r#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::default(),
            },
        }
    }

    fn added_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(proto_job_created(id).into()),
        }
    }

    fn rrule_added_event(id: &str) -> v1::ScheduleEvent {
        let mut created = proto_job_created(id);
        created.schedule = MessageField::some(v1::Schedule {
            kind: Some(
                v1::schedule::RRule {
                    dtstart: MessageField::some(timestamp_from_str("2026-05-24T09:00:00+00:00")),
                    rrule: "FREQ=WEEKLY;BYDAY=MO".to_string(),
                    timezone: MessageField::some(trogonai_proto::google::r#type::TimeZone {
                        id: "UTC".to_string(),
                        ..Default::default()
                    }),
                    rdate: vec![timestamp_from_str("2026-05-26T09:00:00+00:00")],
                    exdate: vec![timestamp_from_str("2026-06-01T09:00:00+00:00")],
                }
                .into(),
            ),
        });
        v1::ScheduleEvent {
            event: Some(created.into()),
        }
    }

    fn proto_job_created(id: &str) -> v1::ScheduleCreated {
        v1::ScheduleCreated {
            schedule_id: id.to_string(),
            status: MessageField::some(v1::ScheduleStatus {
                kind: Some(v1::schedule_status::Scheduled {}.into()),
            }),
            schedule: MessageField::some(v1::Schedule {
                kind: Some(
                    v1::schedule::Every {
                        every: MessageField::some(Duration {
                            seconds: 30,
                            ..Default::default()
                        }),
                    }
                    .into(),
                ),
            }),
            delivery: MessageField::some(v1::Delivery {
                kind: Some(
                    v1::delivery::NatsMessage {
                        subject: "agent.run".to_string(),
                        ttl: MessageField::none(),
                        source: MessageField::none(),
                    }
                    .into(),
                ),
            }),
            message: MessageField::some(v1::Message {
                content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
                    content_type: "application/json".to_string(),
                    data: r#"{"kind":"heartbeat"}"#.as_bytes().to_vec(),
                }),
                headers: Vec::new(),
            }),
        }
    }

    fn paused_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::SchedulePaused {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    fn removed_event(id: &str) -> v1::ScheduleEvent {
        v1::ScheduleEvent {
            event: Some(
                v1::ScheduleRemoved {
                    schedule_id: id.to_string(),
                }
                .into(),
            ),
        }
    }

    #[test]
    fn event_projection_replays_latest_state() {
        let events = [added_event("backup"), paused_event("backup"), removed_event("backup")];
        let mut state = initial_state();

        for event in &events {
            state = apply("backup", state, event).unwrap();
        }

        assert_eq!(state, ScheduleStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn event_projection_preserves_rrule_schedule_fields() {
        let state = apply("backup", initial_state(), &rrule_added_event("backup")).unwrap();
        let ScheduleStreamState::Present(job) = state else {
            panic!("expected projected job");
        };

        let dtstart: DateTime<Utc> = "2026-05-24T09:00:00+00:00".parse().unwrap();
        let rdate: DateTime<Utc> = "2026-05-26T09:00:00+00:00".parse().unwrap();
        let exdate: DateTime<Utc> = "2026-06-01T09:00:00+00:00".parse().unwrap();

        assert_eq!(
            job.schedule,
            ScheduleEventSchedule::RRule {
                dtstart,
                rrule: "FREQ=WEEKLY;BYDAY=MO".to_string(),
                timezone: Some("UTC".to_string()),
                rdate: vec![rdate],
                exdate: vec![exdate],
            }
        );
    }

    #[test]
    fn event_projection_rejects_recreating_deleted_job() {
        let error = apply(
            "backup",
            ScheduleStreamState::Deleted("backup".to_string()),
            &added_event("backup"),
        )
        .unwrap_err();

        assert!(matches!(
            error,
            ScheduleTransitionError::CannotAddDeletedSchedule { .. }
        ));
    }

    #[test]
    fn state_change_requires_existing_job() {
        let error = apply("missing", initial_state(), &paused_event("missing")).unwrap_err();

        assert!(matches!(
            error,
            ScheduleTransitionError::MissingScheduleForStateChange { .. }
        ));
    }

    #[test]
    fn projection_change_tracks_latest_state() {
        let before = initial_state();
        let after = apply("backup", before.clone(), &added_event("backup")).unwrap();
        assert_eq!(
            projection_change(&before, &after),
            Some(ProjectionChange::Upsert(expected_schedule("backup")))
        );

        let updated = apply("backup", after.clone(), &paused_event("backup")).unwrap();
        match projection_change(&after, &updated).unwrap() {
            ProjectionChange::Upsert(job) => assert_eq!(job.status, ScheduleEventStatus::Paused),
            ProjectionChange::Delete(_) => panic!("expected upsert change"),
        }
    }

    #[test]
    fn initial_state_rejects_adding_existing_job() {
        let error = apply(
            "backup",
            ScheduleStreamState::Present(expected_schedule("backup")),
            &added_event("backup"),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            ScheduleTransitionError::CannotAddExistingSchedule { .. }
        ));
    }

    #[test]
    fn initial_removal_creates_deleted_tombstone() {
        let state = apply("backup", initial_state(), &removed_event("backup")).unwrap();
        assert_eq!(state, ScheduleStreamState::Deleted("backup".to_string()));
    }

    #[test]
    fn read_model_state_rejects_recreating_deleted_job() {
        let mut states = BTreeMap::new();
        let stream_id = "alpha".to_string();

        apply_event_to_read_model_state(&mut states, &stream_id, &added_event("alpha")).unwrap();
        apply_event_to_read_model_state(&mut states, &stream_id, &paused_event("alpha")).unwrap();
        apply_event_to_read_model_state(&mut states, &stream_id, &removed_event("alpha")).unwrap();
        let error = apply_event_to_read_model_state(&mut states, &stream_id, &added_event("alpha")).unwrap_err();

        assert!(error.to_string().contains("deleted"));
        assert_eq!(states.get("alpha"), Some(&ScheduleStreamState::Deleted(stream_id)));
    }

    #[test]
    fn read_model_state_rejects_invalid_transition_sequence() {
        let stream_id = "alpha".to_string();
        let error =
            apply_event_to_read_model_state(&mut BTreeMap::new(), &stream_id, &paused_event("alpha")).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("failed to apply schedule event to schedules read model")
        );
    }
}
