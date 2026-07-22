use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use buffa::MessageField;
use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, TimeZone, Utc};
use trogon_decider_runtime::snapshot::Snapshot;
use trogon_decider_runtime::{
    AppendStreamRequest, AppendStreamResponse, Event, EventData, EventDecode, EventEncode, EventId, EventIdentity,
    EventType, Headers, ReadFrom, ReadSnapshotRequest, ReadSnapshotResponse, ReadStreamRequest, ReadStreamResponse,
    SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode, SnapshotRead, SnapshotType, SnapshotWrite,
    StreamAppend, StreamEvent, StreamPosition, StreamRead, StreamWritePrecondition, WriteSnapshotRequest,
    WriteSnapshotResponse,
};
use trogon_std::{NowV7, UuidV7Generator};

use crate::{
    DeliveryKind, GetSchedule, ListSchedules, ScheduleEventCase, ScheduleKind, ScheduleStatusKind, SourceKind,
    config::{ScheduleWriteCondition, ScheduleWriteState},
    error::SchedulerError,
    queries::read_model::{
        MessageContent, MessageEnvelope, MessageHeaders, Schedule, ScheduleEventDelivery, ScheduleEventSamplingSource,
        ScheduleEventSchedule, ScheduleEventStatus,
    },
    v1,
};

#[derive(Clone, Default)]
pub struct MockSchedulerStore {
    schedules: Arc<Mutex<HashMap<String, Schedule>>>,
    stream_positions: Arc<Mutex<HashMap<String, StreamPosition>>>,
    events: Arc<Mutex<HashMap<String, Vec<Event>>>>,
    command_snapshots: Arc<Mutex<HashMap<String, HashMap<String, EncodedSnapshot>>>>,
}

#[derive(Clone)]
struct EncodedSnapshot {
    position: StreamPosition,
    payload: Vec<u8>,
}

fn stream_position(value: u64) -> Result<StreamPosition, SchedulerError> {
    StreamPosition::try_new(value)
        .map_err(|source| SchedulerError::event_source("mock stream position must be non-zero", source))
}

fn encode_event<E>(event: &E) -> Event
where
    E: EventType + EventIdentity + EventEncode,
    <E as EventType>::Error: std::fmt::Debug,
    <E as EventEncode>::Error: std::fmt::Debug,
{
    let id = event
        .event_id()
        .unwrap_or_else(|| EventId::new(UuidV7Generator.now_v7()));
    Event {
        id,
        r#type: event.event_type().unwrap().to_string(),
        content: event.encode().unwrap(),
        headers: Headers::empty(),
    }
}

impl MockSchedulerStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn seed_schedule(&self, job: Schedule) {
        let id = job.id.clone();
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleCreated {
                    schedule_id: job.id.clone(),
                    status: MessageField::some(v1::ScheduleStatus {
                        kind: Some(match job.status {
                            ScheduleEventStatus::Scheduled => v1::schedule_status::Scheduled {}.into(),
                            ScheduleEventStatus::Paused => v1::schedule_status::Paused {}.into(),
                        }),
                    }),
                    schedule: MessageField::some(proto_schedule(&job.schedule)),
                    delivery: MessageField::some(proto_delivery(&job.delivery)),
                    message: MessageField::some(proto_message(&job.message)),
                }
                .into(),
            ),
        };

        let initial_position = StreamPosition::new(NonZeroU64::MIN);
        self.stream_positions
            .lock()
            .unwrap()
            .insert(id.clone(), initial_position);
        self.events
            .lock()
            .unwrap()
            .insert(id.clone(), vec![encode_event(&event)]);
        self.schedules.lock().unwrap().insert(id.clone(), job);
    }

    pub(crate) fn read_command_snapshot<Payload>(
        &self,
        snapshot_id: &(impl AsRef<str> + ?Sized),
    ) -> Result<Option<Snapshot<Payload>>, SchedulerError>
    where
        Payload: SnapshotPayloadDecode + SnapshotType,
        <Payload as SnapshotPayloadDecode>::Error: std::error::Error + Send + Sync + 'static,
        <Payload as SnapshotType>::Error: std::error::Error + Send + Sync + 'static,
    {
        let snapshot_type = Payload::snapshot_type()
            .map_err(|source| SchedulerError::event_source("failed to resolve command snapshot type", source))?;
        self.command_snapshots
            .lock()
            .unwrap()
            .get(snapshot_type.as_ref())
            .and_then(|snapshots| snapshots.get(snapshot_id.as_ref()).cloned())
            .map(|snapshot| {
                Payload::decode(SnapshotPayloadData::new(snapshot.payload.as_slice()))
                    .map(|payload| Snapshot::new(snapshot.position, payload))
                    .map_err(|source| SchedulerError::event_source("failed to decode command snapshot payload", source))
            })
            .transpose()
    }

    pub async fn get_schedule(&self, command: GetSchedule) -> Result<Option<Schedule>, SchedulerError> {
        Ok(self.schedules.lock().unwrap().get(command.id.as_str()).cloned())
    }

    pub async fn list_schedules(&self, _command: ListSchedules) -> Result<Vec<Schedule>, SchedulerError> {
        Ok(self.schedules.lock().unwrap().values().cloned().collect())
    }
}

fn proto_schedule(schedule: &ScheduleEventSchedule) -> v1::Schedule {
    let kind = match schedule {
        ScheduleEventSchedule::At { at } => {
            let ts = Timestamp {
                seconds: at.timestamp(),
                nanos: at.timestamp_subsec_nanos() as i32,
                ..Default::default()
            };
            v1::schedule::At {
                at: MessageField::some(ts),
            }
            .into()
        }
        ScheduleEventSchedule::Every { every } => v1::schedule::Every {
            every: MessageField::some(Duration {
                seconds: every.as_secs() as i64,
                nanos: every.subsec_nanos() as i32,
                ..Default::default()
            }),
        }
        .into(),
        ScheduleEventSchedule::Cron { expr, timezone } => v1::schedule::Cron {
            expr: expr.clone(),
            timezone: timezone
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|tz| trogonai_proto::google::r#type::TimeZone {
                    id: tz.to_string(),
                    ..Default::default()
                })
                .map(MessageField::some)
                .unwrap_or_else(MessageField::none),
        }
        .into(),
        ScheduleEventSchedule::RRule {
            dtstart,
            rrule,
            timezone,
            rdate,
            exdate,
        } => {
            let to_ts = |dt: &chrono::DateTime<chrono::Utc>| Timestamp {
                seconds: dt.timestamp(),
                nanos: dt.timestamp_subsec_nanos() as i32,
                ..Default::default()
            };
            v1::schedule::RRule {
                dtstart: MessageField::some(to_ts(dtstart)),
                rrule: rrule.clone(),
                timezone: timezone
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .map(|tz| trogonai_proto::google::r#type::TimeZone {
                        id: tz.to_string(),
                        ..Default::default()
                    })
                    .map(MessageField::some)
                    .unwrap_or_else(MessageField::none),
                rdate: rdate.iter().map(to_ts).collect(),
                exdate: exdate.iter().map(to_ts).collect(),
            }
            .into()
        }
    };
    v1::Schedule { kind: Some(kind) }
}

fn proto_delivery(delivery: &ScheduleEventDelivery) -> v1::Delivery {
    match delivery {
        ScheduleEventDelivery::NatsMessage { subject, ttl, source } => v1::Delivery {
            kind: Some(
                v1::delivery::NatsMessage {
                    subject: subject.clone(),
                    ttl: ttl
                        .map(|d| Duration {
                            seconds: d.as_secs() as i64,
                            nanos: d.subsec_nanos() as i32,
                            ..Default::default()
                        })
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
                    source: source
                        .as_ref()
                        .map(proto_sampling_source)
                        .map(MessageField::some)
                        .unwrap_or_else(MessageField::none),
                }
                .into(),
            ),
        },
    }
}

fn proto_sampling_source(source: &ScheduleEventSamplingSource) -> v1::delivery::nats_message::Source {
    match source {
        ScheduleEventSamplingSource::LatestFromSubject { subject } => v1::delivery::nats_message::Source {
            kind: Some(
                v1::delivery::nats_message::LatestFromSubject {
                    subject: subject.clone(),
                }
                .into(),
            ),
        },
    }
}

fn proto_message(message: &MessageEnvelope) -> v1::Message {
    v1::Message {
        content: MessageField::some(trogonai_proto::content::v1alpha1::Content {
            content_type: message.content.content_type().to_string(),
            data: message.content.as_str().as_bytes().to_vec(),
        }),
        headers: message
            .headers
            .as_slice()
            .iter()
            .map(|(name, value)| v1::Header {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
    }
}

fn schedule_read_model_from_proto(stream_id: &str, details: &v1::ScheduleCreated) -> Schedule {
    Schedule {
        id: stream_id.to_string(),
        completed: false,
        next_occurrence_at: None,
        last_occurrence_at: None,
        status: {
            let is_paused = matches!(
                details.status.as_option().and_then(|s| s.kind.as_ref()),
                Some(ScheduleStatusKind::Paused(_))
            );
            if is_paused {
                ScheduleEventStatus::Paused
            } else {
                ScheduleEventStatus::Scheduled
            }
        },
        schedule: details
            .schedule
            .as_option()
            .map(schedule_from_proto)
            .unwrap_or(ScheduleEventSchedule::Every {
                every: std::time::Duration::ZERO,
            }),
        delivery: details
            .delivery
            .as_option()
            .map(delivery_from_proto)
            .unwrap_or_else(|| ScheduleEventDelivery::NatsMessage {
                subject: String::new(),
                ttl: None,
                source: None,
            }),
        message: details.message.as_option().map(message_from_proto).unwrap_or_default(),
    }
}

fn schedule_from_proto(schedule: &v1::Schedule) -> ScheduleEventSchedule {
    let ts_to_dt = |ts: &Timestamp| -> chrono::DateTime<chrono::Utc> {
        chrono::Utc
            .timestamp_opt(ts.seconds, ts.nanos as u32)
            .single()
            .unwrap_or_default()
    };
    match schedule.kind.as_ref() {
        Some(ScheduleKind::At(inner)) => ScheduleEventSchedule::At {
            at: inner.at.as_option().map(ts_to_dt).unwrap_or_default(),
        },
        Some(ScheduleKind::Every(inner)) => ScheduleEventSchedule::Every {
            every: inner
                .every
                .as_option()
                .map(|d| std::time::Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32))
                .unwrap_or_default(),
        },
        Some(ScheduleKind::Cron(inner)) => ScheduleEventSchedule::Cron {
            expr: inner.expr.clone(),
            timezone: inner
                .timezone
                .as_option()
                .map(|tz| tz.id.clone())
                .filter(|s| !s.is_empty()),
        },
        Some(ScheduleKind::Rrule(inner)) => ScheduleEventSchedule::RRule {
            dtstart: inner.dtstart.as_option().map(ts_to_dt).unwrap_or_default(),
            rrule: inner.rrule.clone(),
            timezone: inner
                .timezone
                .as_option()
                .map(|tz| tz.id.clone())
                .filter(|s| !s.is_empty()),
            rdate: inner.rdate.iter().map(ts_to_dt).collect(),
            exdate: inner.exdate.iter().map(ts_to_dt).collect(),
        },
        None => ScheduleEventSchedule::Every {
            every: std::time::Duration::ZERO,
        },
    }
}

fn delivery_from_proto(delivery: &v1::Delivery) -> ScheduleEventDelivery {
    match delivery.kind.as_ref() {
        Some(DeliveryKind::NatsMessage(inner)) => ScheduleEventDelivery::NatsMessage {
            subject: inner.subject.clone(),
            ttl: inner
                .ttl
                .as_option()
                .map(|d| std::time::Duration::new(d.seconds.max(0) as u64, d.nanos.max(0) as u32)),
            source: inner.source.as_option().map(sampling_source_from_proto),
        },
        None => ScheduleEventDelivery::NatsMessage {
            subject: String::new(),
            ttl: None,
            source: None,
        },
    }
}

fn sampling_source_from_proto(source: &v1::delivery::nats_message::Source) -> ScheduleEventSamplingSource {
    match source.kind.as_ref() {
        Some(SourceKind::LatestFromSubject(inner)) => ScheduleEventSamplingSource::LatestFromSubject {
            subject: inner.subject.clone(),
        },
        None => ScheduleEventSamplingSource::LatestFromSubject { subject: String::new() },
    }
}

fn message_from_proto(message: &v1::Message) -> MessageEnvelope {
    let content = message
        .content
        .as_option()
        .map(|c| MessageContent::new(c.content_type.clone(), String::from_utf8_lossy(&c.data).into_owned()))
        .unwrap_or_default();
    MessageEnvelope {
        content,
        headers: MessageHeaders::from_pairs(
            message
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone())),
        ),
    }
}

impl StreamRead<str> for MockSchedulerStore {
    type Error = SchedulerError;

    async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
        let stream_id = request.stream_id;
        let from_sequence = match request.from {
            ReadFrom::Beginning => 1,
            ReadFrom::Position(position) => position.as_u64(),
        };
        let current_position = self.stream_positions.lock().unwrap().get(stream_id).copied();
        let stream_events = self.events.lock().unwrap().get(stream_id).cloned().unwrap_or_default();

        let mut recorded = Vec::new();
        for (index, event) in stream_events.into_iter().enumerate() {
            let sequence = index as u64 + 1;
            if sequence < from_sequence {
                continue;
            }
            recorded.push(StreamEvent {
                stream_id: stream_id.to_string(),
                event,
                stream_position: stream_position(sequence)?,
                recorded_at: DateTime::<Utc>::from_timestamp(1_700_000_000 + sequence as i64, 0).ok_or_else(|| {
                    SchedulerError::event_source(
                        "failed to build mocked recorded event timestamp",
                        std::io::Error::other(stream_id.to_string()),
                    )
                })?,
            });
        }
        Ok(ReadStreamResponse {
            current_position,
            events: recorded,
        })
    }
}

impl StreamAppend<str> for MockSchedulerStore {
    type Error = SchedulerError;

    async fn append_stream(&self, request: AppendStreamRequest<'_, str>) -> Result<AppendStreamResponse, Self::Error> {
        let stream_id = request.stream_id.to_string();
        let expected_state = request.stream_write_precondition;
        let events = request.events;
        let jobs = self.schedules.clone();
        let stream_positions = self.stream_positions.clone();
        let event_log = self.events.clone();

        let mut jobs = jobs.lock().unwrap();
        let mut stream_positions = stream_positions.lock().unwrap();
        let mut stream_events = event_log.lock().unwrap();

        let current_job = jobs.get(stream_id.as_str()).cloned();
        let current_position = stream_positions.get(stream_id.as_str()).copied();
        let write_state = ScheduleWriteState::new(current_position, current_position.is_some());
        match expected_state {
            StreamWritePrecondition::Any => {}
            StreamWritePrecondition::StreamExists if write_state.exists() => {}
            StreamWritePrecondition::StreamExists => {
                return Err(SchedulerError::OptimisticConcurrencyConflict {
                    id: stream_id.to_string(),
                    expected: StreamWritePrecondition::StreamExists,
                    current_position,
                });
            }
            StreamWritePrecondition::NoStream => {
                ScheduleWriteCondition::MustNotExist.ensure(stream_id.as_str(), write_state)?;
            }
            StreamWritePrecondition::At(position) => {
                ScheduleWriteCondition::MustBeAtPosition(position).ensure(stream_id.as_str(), write_state)?;
            }
        }

        let stored_events = stream_events.entry(stream_id.to_string()).or_default();
        let mut projected_schedule = current_job;
        let mut raw_position = current_position.map(StreamPosition::as_u64).unwrap_or(0);

        for event_data in events {
            let event = v1::ScheduleEvent::decode(EventData::new(&event_data.r#type, &event_data.content)).map_err(
                |source| SchedulerError::event_source("failed to decode mocked schedule event payload", source),
            )?;
            raw_position += 1;
            stored_events.push(event_data);
            let Some(event) = event.into_decoded() else {
                continue;
            };
            match &event.event {
                Some(ScheduleEventCase::ScheduleCreated(inner)) => {
                    projected_schedule = Some(schedule_read_model_from_proto(stream_id.as_str(), inner));
                }
                Some(ScheduleEventCase::SchedulePaused(_)) => {
                    let mut job = projected_schedule.take().ok_or_else(|| {
                        SchedulerError::event_source(
                            "failed to project mocked schedule pause without current read model",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    job.status = crate::ScheduleEventStatus::Paused;
                    projected_schedule = Some(job);
                }
                Some(ScheduleEventCase::ScheduleResumed(_)) => {
                    let mut job = projected_schedule.take().ok_or_else(|| {
                        SchedulerError::event_source(
                            "failed to project mocked schedule resume without current read model",
                            std::io::Error::other(stream_id.to_string()),
                        )
                    })?;
                    job.status = crate::ScheduleEventStatus::Scheduled;
                    projected_schedule = Some(job);
                }
                Some(ScheduleEventCase::ScheduleRemoved(_)) => {
                    projected_schedule = None;
                }
                Some(ScheduleEventCase::ScheduleCompleted(_)) => {
                    if let Some(job) = projected_schedule.as_mut() {
                        job.completed = true;
                    }
                }
                Some(
                    ScheduleEventCase::ScheduleOccurrenceRecorded(_)
                    | ScheduleEventCase::ScheduleOccurrenceScheduled(_),
                ) => {}
                None => {
                    return Err(SchedulerError::event_source(
                        "failed to project mocked schedule event without supported case",
                        std::io::Error::other("missing event case"),
                    ));
                }
            }
        }

        let final_position = stream_position(raw_position)?;
        stream_positions.insert(stream_id.to_string(), final_position);
        if let Some(job) = projected_schedule {
            jobs.insert(stream_id.to_string(), job);
        } else {
            jobs.remove(stream_id.as_str());
        }
        Ok(AppendStreamResponse {
            stream_position: final_position,
        })
    }
}

impl<Payload> SnapshotRead<Payload, str> for MockSchedulerStore
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
        self.read_command_snapshot(request.snapshot_id)
            .map(|snapshot| ReadSnapshotResponse { snapshot })
    }
}

impl<Payload> SnapshotWrite<Payload, str> for MockSchedulerStore
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
        let snapshot =
            EncodedSnapshot {
                position: request.snapshot.position,
                payload: request.snapshot.payload.encode().map_err(|source| {
                    SchedulerError::event_source("failed to encode command snapshot payload", source)
                })?,
            };
        let snapshot_type = Payload::snapshot_type()
            .map_err(|source| SchedulerError::event_source("failed to resolve command snapshot type", source))?;
        self.command_snapshots
            .lock()
            .unwrap()
            .entry(snapshot_type.to_string())
            .or_default()
            .insert(request.snapshot_id.to_string(), snapshot);
        Ok(WriteSnapshotResponse)
    }
}

#[cfg(test)]
mod tests;
