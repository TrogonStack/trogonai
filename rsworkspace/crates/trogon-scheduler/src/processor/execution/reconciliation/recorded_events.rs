//! Decoding persisted `v1::ScheduleEvent` records into the input reconciled by
//! this component.
//!
//! The command write-side only owns the domain -> proto direction. The
//! processor needs the reverse: it consumes persisted proto payloads and must
//! rebuild the validated domain value objects before reconciling. These
//! conversions are the foundation reused by both the live event stream
//! (slice 4) and the rebuildable KV checkpoint cache (slice 2).

use std::time::Duration;

use buffa_types::google::protobuf::{Duration as ProtoDuration, Timestamp};
use chrono::{DateTime, Utc};
use trogon_decider_runtime::{EventDecodeOutcome, StreamEvent};
use trogonai_proto::scheduler::schedules::{
    DeliveryKind, ScheduleEventCase, ScheduleKind, ScheduleStatusKind, SourceKind, v1,
};

use crate::commands::domain::{
    CronExpression, CronExpressionError, Delivery, DeliveryRoute, DeliveryRouteError, EveryDuration,
    EveryDurationError, MessageContent, RRuleDateTime, RRuleDateTimeError, RRuleExpression, RRuleExpressionError,
    SamplingSource, SamplingSubjectError, Schedule, ScheduleEventStatus, ScheduleHeaders, ScheduleHeadersError,
    ScheduleId, ScheduleIdError, ScheduleMessage, ScheduleOccurrenceSequence, ScheduleOccurrenceSequenceError,
    ScheduleTimezone, TimeZoneError, TtlDuration, TtlDurationError, TzdbVersion, TzdbVersionError,
};

use super::reconcile::{ScheduleChange, ScheduleDefinition};
use super::{ScheduleKey, StreamRoutingId};

pub use trogonai_proto::scheduler::schedules::ScheduleEventPayloadError;

/// Error raised while decoding a persisted schedule event into a domain
/// schedule change.
#[derive(Debug, thiserror::Error)]
pub enum ScheduleEventDecodeError {
    /// The `ScheduleEvent` envelope carried no `oneof` case.
    #[error("schedule event envelope carried no event")]
    MissingEvent,
    /// A required nested proto field was absent.
    #[error("schedule event is missing required field '{field}'")]
    MissingField { field: &'static str },
    /// The persisted schedule id is no longer a valid [`ScheduleId`].
    #[error("schedule id is invalid: {source}")]
    ScheduleId {
        #[source]
        source: ScheduleIdError,
    },
    /// The stream envelope id does not match the payload schedule id.
    #[error("stream id '{stream_id}' does not match payload schedule id '{schedule_id}'")]
    StreamRoutingMismatch { stream_id: String, schedule_id: String },
    /// A persisted timestamp could not be represented as a UTC instant.
    #[error("timestamp field '{field}' is out of range")]
    Timestamp { field: &'static str },
    /// A persisted duration was negative or otherwise unrepresentable.
    #[error("duration field '{field}' is out of range")]
    Duration { field: &'static str },
    /// The persisted `every` interval is no longer valid.
    #[error("every duration is invalid: {source}")]
    EveryDuration {
        #[source]
        source: EveryDurationError,
    },
    /// The persisted delivery TTL is no longer valid.
    #[error("ttl duration is invalid: {source}")]
    TtlDuration {
        #[source]
        source: TtlDurationError,
    },
    /// The persisted cron expression is no longer valid.
    #[error("cron expression is invalid: {source}")]
    CronExpression {
        #[source]
        source: CronExpressionError,
    },
    /// A persisted RRULE timestamp is no longer valid.
    #[error("rrule datetime is invalid: {source}")]
    RRuleDateTime {
        #[source]
        source: RRuleDateTimeError,
    },
    /// The persisted RRULE expression is no longer valid.
    #[error("rrule expression is invalid: {source}")]
    RRuleExpression {
        #[source]
        source: RRuleExpressionError,
    },
    /// A persisted time zone is no longer valid.
    #[error("time zone is invalid: {source}")]
    TimeZone {
        #[source]
        source: TimeZoneError,
    },
    /// A persisted tzdb version is no longer valid.
    #[error("tzdb version is invalid: {source}")]
    TzdbVersion {
        #[source]
        source: TzdbVersionError,
    },
    /// The persisted delivery route is no longer a valid NATS subject token.
    #[error("delivery route is invalid: {source}")]
    DeliveryRoute {
        #[source]
        source: DeliveryRouteError,
    },
    /// The persisted sampling subject is no longer valid.
    #[error("sampling subject is invalid: {source}")]
    SamplingSubject {
        #[source]
        source: SamplingSubjectError,
    },
    /// The persisted message payload was not valid UTF-8.
    #[error("message content was not valid UTF-8")]
    ContentNotUtf8,
    /// The persisted user headers are no longer valid.
    #[error("message headers are invalid: {source}")]
    Headers {
        #[source]
        source: ScheduleHeadersError,
    },
    /// The proto payload itself could not be decoded.
    #[error("schedule event payload could not be decoded: {source}")]
    Payload {
        #[source]
        source: ScheduleEventPayloadError,
    },
    /// The persisted occurrence sequence is no longer valid.
    #[error("schedule occurrence sequence is invalid: {source}")]
    OccurrenceSequence {
        #[source]
        source: ScheduleOccurrenceSequenceError,
    },
}

/// Decodes the schedule event payload of a [`StreamEvent`].
///
/// Returns `Ok(None)` when the stored envelope belongs to another decider's
/// event set, mirroring [`EventDecodeOutcome::Skipped`]. The stream id is the
/// original `schedule_id`, but it is only used as a fallback: the schedule
/// change derives its `schedule_id` from the persisted payload so the two stay
/// consistent.
pub fn schedule_change_from_stream_event(
    stream_event: &StreamEvent,
) -> Result<Option<ScheduleChange>, ScheduleEventDecodeError> {
    let outcome = stream_event
        .decode::<v1::ScheduleEvent>()
        .map_err(|source| ScheduleEventDecodeError::Payload { source })?;

    match outcome {
        EventDecodeOutcome::Decoded(event) => decode_schedule_change(&event).map(Some),
        EventDecodeOutcome::Skipped => Ok(None),
    }
}

fn stream_lane_key(stream_event: &StreamEvent) -> ScheduleKey {
    ScheduleKey::for_stream(&StreamRoutingId::from(stream_event.stream_id.as_str()))
}

/// The payload decode threaded from lane routing to the worker so one record is
/// protobuf-decoded once, not once per phase.
#[derive(Debug)]
pub enum DecodedScheduleEvent {
    /// The payload decoded into a schedule change.
    Change(ScheduleChange),
    /// The envelope belongs to another decider's event set.
    Foreign,
    /// No decoded payload is available (decode failed, panicked, or was never
    /// attempted); the worker decodes inside its own panic boundary to produce
    /// the durable failure record.
    Undecoded,
}

/// Derives the aggregate lane key for one delivered stream record, alongside
/// the decoded payload so the worker does not decode the same bytes again.
///
/// Decodable schedule events route by the payload [`ScheduleId`] so lane
/// serialization matches checkpoint load/save. Foreign or undecodable records
/// fall back to the stream id so they still land on a deterministic lane.
pub fn lane_route_from_stream_event(stream_event: &StreamEvent) -> (ScheduleKey, DecodedScheduleEvent) {
    // The dispatcher derives lane keys outside its per-message panic boundary,
    // so a payload-decode panic here must not escape and kill every lane.
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        schedule_change_from_stream_event(stream_event)
    }));
    match decoded {
        Ok(Ok(Some(change))) => (
            ScheduleKey::derive(change.schedule_id()),
            DecodedScheduleEvent::Change(change),
        ),
        Ok(Ok(None)) => (stream_lane_key(stream_event), DecodedScheduleEvent::Foreign),
        Ok(Err(error)) => {
            tracing::warn!(
                stream_id = %stream_event.stream_id,
                stream_position = %stream_event.stream_position,
                error = %error,
                "schedule event payload failed to decode; routing lane by stream id"
            );
            (stream_lane_key(stream_event), DecodedScheduleEvent::Undecoded)
        }
        Err(_) => {
            tracing::error!(
                stream_id = %stream_event.stream_id,
                stream_position = %stream_event.stream_position,
                "schedule event payload decode panicked; routing lane by stream id"
            );
            (stream_lane_key(stream_event), DecodedScheduleEvent::Undecoded)
        }
    }
}

/// Returns whether the stream envelope id resolves to the same lane as the
/// decoded payload schedule id.
pub fn stream_routing_matches_payload(stream_event: &StreamEvent, change: &ScheduleChange) -> bool {
    ScheduleKey::derive(change.schedule_id()) == stream_lane_key(stream_event)
}

/// Decodes a fully materialized `v1::ScheduleEvent` into a schedule change.
pub fn decode_schedule_change(event: &v1::ScheduleEvent) -> Result<ScheduleChange, ScheduleEventDecodeError> {
    match event.event.as_ref().ok_or(ScheduleEventDecodeError::MissingEvent)? {
        ScheduleEventCase::ScheduleCreated(created) => {
            let schedule_id = schedule_id_from(&created.schedule_id)?;
            let definition = definition_from_created(created)?;
            Ok(ScheduleChange::Created {
                schedule_id,
                definition: Box::new(definition),
            })
        }
        ScheduleEventCase::SchedulePaused(paused) => Ok(ScheduleChange::Paused {
            schedule_id: schedule_id_from(&paused.schedule_id)?,
        }),
        ScheduleEventCase::ScheduleResumed(resumed) => Ok(ScheduleChange::Resumed {
            schedule_id: schedule_id_from(&resumed.schedule_id)?,
        }),
        ScheduleEventCase::ScheduleRemoved(removed) => Ok(ScheduleChange::Removed {
            schedule_id: schedule_id_from(&removed.schedule_id)?,
        }),
        ScheduleEventCase::ScheduleOccurrenceRecorded(recorded) => {
            let schedule_id = schedule_id_from(&recorded.schedule_id)?;
            let occurrence_sequence = ScheduleOccurrenceSequence::try_new(recorded.occurrence_sequence.ok_or(
                ScheduleEventDecodeError::MissingField {
                    field: "occurrence_sequence",
                },
            )?)
            .map_err(|source| ScheduleEventDecodeError::OccurrenceSequence { source })?;
            let occurrence_at = timestamp_to_datetime(
                recorded
                    .occurrence_at
                    .as_option()
                    .ok_or(ScheduleEventDecodeError::MissingField { field: "occurrence_at" })?,
                "occurrence_at",
            )?;
            Ok(ScheduleChange::OccurrenceRecorded {
                schedule_id,
                occurrence_sequence,
                occurrence_at,
            })
        }
        ScheduleEventCase::ScheduleOccurrenceScheduled(scheduled) => {
            let schedule_id = schedule_id_from(&scheduled.schedule_id)?;
            let occurrence_at = timestamp_to_datetime(
                scheduled
                    .occurrence_at
                    .as_option()
                    .ok_or(ScheduleEventDecodeError::MissingField { field: "occurrence_at" })?,
                "occurrence_at",
            )?;
            Ok(ScheduleChange::OccurrenceScheduled {
                schedule_id,
                occurrence_at,
            })
        }
        ScheduleEventCase::ScheduleCompleted(completed) => Ok(ScheduleChange::Completed {
            schedule_id: schedule_id_from(&completed.schedule_id)?,
        }),
    }
}

fn definition_from_created(created: &v1::ScheduleCreated) -> Result<ScheduleDefinition, ScheduleEventDecodeError> {
    let status = status_from_proto(created.status.as_option());
    let schedule = schedule_from_proto(
        created
            .schedule
            .as_option()
            .ok_or(ScheduleEventDecodeError::MissingField { field: "schedule" })?,
    )?;
    let delivery = delivery_from_proto(
        created
            .delivery
            .as_option()
            .ok_or(ScheduleEventDecodeError::MissingField { field: "delivery" })?,
    )?;
    let message = message_from_proto(
        created
            .message
            .as_option()
            .ok_or(ScheduleEventDecodeError::MissingField { field: "message" })?,
    )?;

    Ok(ScheduleDefinition {
        status,
        schedule,
        delivery,
        message,
    })
}

pub(crate) fn schedule_id_from(raw: &str) -> Result<ScheduleId, ScheduleEventDecodeError> {
    ScheduleId::parse(raw).map_err(|source| ScheduleEventDecodeError::ScheduleId { source })
}

pub(crate) fn status_from_proto(status: Option<&v1::ScheduleStatus>) -> ScheduleEventStatus {
    match status.and_then(|status| status.kind.as_ref()) {
        Some(ScheduleStatusKind::Paused(_)) => ScheduleEventStatus::Paused,
        Some(ScheduleStatusKind::Scheduled(_)) | None => ScheduleEventStatus::Scheduled,
    }
}

pub(crate) fn schedule_from_proto(schedule: &v1::Schedule) -> Result<Schedule, ScheduleEventDecodeError> {
    match schedule
        .kind
        .as_ref()
        .ok_or(ScheduleEventDecodeError::MissingField { field: "schedule.kind" })?
    {
        ScheduleKind::At(at) => {
            let at = timestamp_to_datetime(
                at.at
                    .as_option()
                    .ok_or(ScheduleEventDecodeError::MissingField { field: "at" })?,
                "at",
            )?;
            Ok(Schedule::At { at })
        }
        ScheduleKind::Every(every) => {
            let duration = proto_duration_to_std(
                every
                    .every
                    .as_option()
                    .ok_or(ScheduleEventDecodeError::MissingField { field: "every" })?,
                "every",
            )?;
            let every =
                EveryDuration::new(duration).map_err(|source| ScheduleEventDecodeError::EveryDuration { source })?;
            Ok(Schedule::Every { every })
        }
        ScheduleKind::Cron(cron) => {
            let expr = CronExpression::new(cron.expr.clone())
                .map_err(|source| ScheduleEventDecodeError::CronExpression { source })?;
            let timezone = timezone_from_proto(cron.timezone.as_option())?;
            Ok(Schedule::Cron { expr, timezone })
        }
        ScheduleKind::Rrule(rrule) => {
            let dtstart = rrule_datetime_from(
                "dtstart",
                rrule
                    .dtstart
                    .as_option()
                    .ok_or(ScheduleEventDecodeError::MissingField { field: "dtstart" })?,
            )?;
            let expression = RRuleExpression::new(rrule.rrule.clone())
                .map_err(|source| ScheduleEventDecodeError::RRuleExpression { source })?;
            let timezone = timezone_from_proto(rrule.timezone.as_option())?;
            let rdate = rrule
                .rdate
                .iter()
                .map(|timestamp| rrule_datetime_from("rdate", timestamp))
                .collect::<Result<Vec<_>, _>>()?;
            let exdate = rrule
                .exdate
                .iter()
                .map(|timestamp| rrule_datetime_from("exdate", timestamp))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Schedule::RRule {
                dtstart,
                rrule: expression,
                timezone,
                rdate,
                exdate,
            })
        }
    }
}

pub(crate) fn delivery_from_proto(delivery: &v1::Delivery) -> Result<Delivery, ScheduleEventDecodeError> {
    match delivery
        .kind
        .as_ref()
        .ok_or(ScheduleEventDecodeError::MissingField { field: "delivery.kind" })?
    {
        DeliveryKind::NatsMessage(nats) => {
            let route = DeliveryRoute::new(&nats.subject)
                .map_err(|source| ScheduleEventDecodeError::DeliveryRoute { source })?;
            let ttl = match nats.ttl.as_option() {
                Some(ttl) => {
                    let duration = proto_duration_to_std(ttl, "ttl")?;
                    Some(
                        TtlDuration::new(duration)
                            .map_err(|source| ScheduleEventDecodeError::TtlDuration { source })?,
                    )
                }
                None => None,
            };
            let source = match nats.source.as_option().and_then(|source| source.kind.as_ref()) {
                Some(SourceKind::LatestFromSubject(latest)) => Some(
                    SamplingSource::latest_from_subject(&latest.subject)
                        .map_err(|source| ScheduleEventDecodeError::SamplingSubject { source })?,
                ),
                None => None,
            };
            Ok(Delivery::NatsEvent { route, ttl, source })
        }
    }
}

pub(crate) fn message_from_proto(message: &v1::Message) -> Result<ScheduleMessage, ScheduleEventDecodeError> {
    let content = match message.content.as_option() {
        Some(content) => {
            let data = String::from_utf8(content.data.clone()).map_err(|_| ScheduleEventDecodeError::ContentNotUtf8)?;
            MessageContent::with_content_type(data, content.content_type.clone())
        }
        None => MessageContent::default(),
    };

    let headers = ScheduleHeaders::new(
        message
            .headers
            .iter()
            .map(|header| (header.name.clone(), header.value.clone())),
    )
    .map_err(|source| ScheduleEventDecodeError::Headers { source })?;

    Ok(ScheduleMessage { content, headers })
}

fn timezone_from_proto(
    timezone: Option<&trogonai_proto::google::r#type::TimeZone>,
) -> Result<Option<ScheduleTimezone>, ScheduleEventDecodeError> {
    let Some(timezone) = timezone.filter(|timezone| !timezone.id.is_empty()) else {
        return Ok(None);
    };

    let zone = if timezone.version.is_empty() {
        ScheduleTimezone::new(&timezone.id).map_err(|source| ScheduleEventDecodeError::TimeZone { source })?
    } else {
        let version =
            TzdbVersion::new(&timezone.version).map_err(|source| ScheduleEventDecodeError::TzdbVersion { source })?;
        ScheduleTimezone::with_tzdb_version(&timezone.id, version)
            .map_err(|source| ScheduleEventDecodeError::TimeZone { source })?
    };
    Ok(Some(zone))
}

fn rrule_datetime_from(field: &'static str, timestamp: &Timestamp) -> Result<RRuleDateTime, ScheduleEventDecodeError> {
    let datetime = timestamp_to_datetime(timestamp, field)?;
    RRuleDateTime::new(field, datetime.to_rfc3339())
        .map_err(|source| ScheduleEventDecodeError::RRuleDateTime { source })
}

fn timestamp_to_datetime(
    timestamp: &Timestamp,
    field: &'static str,
) -> Result<DateTime<Utc>, ScheduleEventDecodeError> {
    trogonai_proto::convert::datetime_from_timestamp(timestamp)
        .map_err(|_| ScheduleEventDecodeError::Timestamp { field })
}

fn proto_duration_to_std(duration: &ProtoDuration, field: &'static str) -> Result<Duration, ScheduleEventDecodeError> {
    trogonai_proto::convert::std_from_duration(duration).map_err(|_| ScheduleEventDecodeError::Duration { field })
}

#[cfg(test)]
mod tests {
    use buffa::MessageField;

    use super::*;
    use crate::commands::domain::MessageEnvelope;
    use crate::commands::domain::{
        Delivery as DomainDelivery, MessageContent as DomainContent, RRuleTimezone, SamplingSource as DomainSource,
        Schedule as DomainSchedule, ScheduleEventDelivery, ScheduleEventSchedule, ScheduleHeaders as DomainHeaders,
        ScheduleMessage as DomainMessage,
    };

    fn at_instant() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2027-03-04T05:06:07Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn proto_schedule(schedule: &DomainSchedule) -> v1::Schedule {
        v1::Schedule::try_from(&ScheduleEventSchedule::from(schedule)).unwrap()
    }

    fn proto_delivery(delivery: &DomainDelivery) -> v1::Delivery {
        v1::Delivery::try_from(&ScheduleEventDelivery::from(delivery)).unwrap()
    }

    fn proto_message(message: &DomainMessage) -> v1::Message {
        v1::Message::from(&MessageEnvelope::from(message))
    }

    #[test]
    fn schedule_round_trips_through_proto_for_every_kind() {
        let schedules = [
            DomainSchedule::At { at: at_instant() },
            DomainSchedule::every(Duration::from_secs(90)).unwrap(),
            DomainSchedule::cron("0 0 * * * *", Some("America/New_York".to_string())).unwrap(),
            DomainSchedule::cron("0 0 * * * *", None).unwrap(),
        ];

        for schedule in schedules {
            let decoded = schedule_from_proto(&proto_schedule(&schedule)).unwrap();
            assert_eq!(decoded, schedule);
        }
    }

    #[test]
    fn rrule_round_trips_through_proto() {
        let rrule =
            DomainSchedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", Some("UTC".to_string())).unwrap();
        let decoded = schedule_from_proto(&proto_schedule(&rrule)).unwrap();

        let DomainSchedule::RRule {
            rrule: a,
            timezone: tz_a,
            ..
        } = decoded
        else {
            panic!("expected decoded RRule schedule");
        };
        let DomainSchedule::RRule {
            rrule: b,
            timezone: tz_b,
            ..
        } = rrule
        else {
            panic!("expected source RRule schedule");
        };
        assert_eq!(a.as_str(), b.as_str());
        assert_eq!(
            tz_a.map(|tz| tz.as_str().to_string()),
            tz_b.map(|tz| tz.as_str().to_string())
        );
        let _ = RRuleTimezone::new("UTC").unwrap();
    }

    #[test]
    fn delivery_round_trips_with_ttl_and_source() {
        let delivery = DomainDelivery::NatsEvent {
            route: DeliveryRoute::new("agent.run").unwrap(),
            ttl: Some(TtlDuration::from_secs(60).unwrap()),
            source: Some(DomainSource::latest_from_subject("agent.events").unwrap()),
        };
        let decoded = delivery_from_proto(&proto_delivery(&delivery)).unwrap();
        assert_eq!(decoded, delivery);
    }

    #[test]
    fn message_round_trips_content_type_and_headers() {
        let message = DomainMessage {
            content: DomainContent::json(r#"{"ok":true}"#),
            headers: DomainHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        };
        let decoded = message_from_proto(&proto_message(&message)).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn created_event_missing_schedule_field_is_rejected() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
            schedule: MessageField::none(),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };
        let event = v1::ScheduleEvent {
            event: Some(created.into()),
        };

        let error = decode_schedule_change(&event).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "schedule" }
        ));
    }

    #[test]
    fn schedule_at_missing_timestamp_is_rejected() {
        use trogonai_proto::scheduler::schedules::v1::schedule;
        let schedule = v1::Schedule {
            kind: Some(v1::schedule::Kind::At(Box::new(schedule::At {
                at: MessageField::none(),
            }))),
        };
        let error = schedule_from_proto(&schedule).unwrap_err();
        assert!(matches!(error, ScheduleEventDecodeError::MissingField { field: "at" }));
    }

    #[test]
    fn message_without_content_defaults_payload() {
        let message = v1::Message {
            content: MessageField::none(),
            headers: Vec::new(),
        };
        let decoded = message_from_proto(&message).unwrap();
        assert_eq!(decoded.content, DomainContent::default());
    }

    #[test]
    fn created_event_missing_delivery_field_is_rejected() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::none(),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };
        let event = v1::ScheduleEvent {
            event: Some(created.into()),
        };

        let error = decode_schedule_change(&event).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "delivery" }
        ));
    }

    #[test]
    fn created_event_missing_message_field_is_rejected() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::none(),
        };
        let event = v1::ScheduleEvent {
            event: Some(created.into()),
        };

        let error = decode_schedule_change(&event).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "message" }
        ));
    }

    #[test]
    fn schedule_every_missing_duration_is_rejected() {
        use trogonai_proto::scheduler::schedules::v1::schedule;
        let schedule = v1::Schedule {
            kind: Some(v1::schedule::Kind::Every(Box::new(schedule::Every {
                every: MessageField::none(),
            }))),
        };
        let error = schedule_from_proto(&schedule).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "every" }
        ));
    }

    #[test]
    fn schedule_rrule_missing_dtstart_is_rejected() {
        use trogonai_proto::scheduler::schedules::v1::schedule;
        let schedule = v1::Schedule {
            kind: Some(v1::schedule::Kind::Rrule(Box::new(schedule::RRule {
                dtstart: MessageField::none(),
                rrule: "FREQ=DAILY;COUNT=1".to_string(),
                timezone: MessageField::none(),
                rdate: Vec::new(),
                exdate: Vec::new(),
            }))),
        };
        let error = schedule_from_proto(&schedule).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "dtstart" }
        ));
    }

    #[test]
    fn timezone_with_version_round_trips() {
        let timezone = trogonai_proto::google::r#type::TimeZone {
            id: "America/New_York".to_string(),
            version: "2025b".to_string(),
        };
        let decoded = timezone_from_proto(Some(&timezone)).unwrap().expect("timezone");
        assert_eq!(decoded.as_str(), "America/New_York");
    }

    #[test]
    fn schedule_proto_kinds_decode_successfully() {
        let at = schedule_from_proto(&proto_schedule(&DomainSchedule::At { at: at_instant() })).unwrap();
        assert!(matches!(at, DomainSchedule::At { .. }));

        let every =
            schedule_from_proto(&proto_schedule(&DomainSchedule::every(Duration::from_secs(5)).unwrap())).unwrap();
        assert!(matches!(every, DomainSchedule::Every { .. }));

        let cron = schedule_from_proto(&proto_schedule(&DomainSchedule::cron("0 0 * * * *", None).unwrap())).unwrap();
        assert!(matches!(cron, DomainSchedule::Cron { .. }));

        let rrule = schedule_from_proto(&proto_schedule(
            &DomainSchedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=1", Some("UTC".to_string())).unwrap(),
        ))
        .unwrap();
        assert!(matches!(rrule, DomainSchedule::RRule { .. }));
    }

    #[test]
    fn definition_from_created_rejects_missing_schedule() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::none(),
            schedule: MessageField::none(),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };

        let error = definition_from_created(&created).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "schedule" }
        ));
    }

    #[test]
    fn definition_from_created_rejects_missing_delivery() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::none(),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::none(),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };

        let error = definition_from_created(&created).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "delivery" }
        ));
    }

    #[test]
    fn definition_from_created_rejects_missing_message() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::none(),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::none(),
        };

        let error = definition_from_created(&created).unwrap_err();
        assert!(matches!(
            error,
            ScheduleEventDecodeError::MissingField { field: "message" }
        ));
    }

    #[test]
    fn definition_from_created_decodes_all_fields() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Paused)),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };

        let definition = definition_from_created(&created).unwrap();
        assert_eq!(definition.status, ScheduleEventStatus::Paused);
    }

    #[test]
    fn created_event_decodes_into_a_schedule_change() {
        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Paused)),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };
        let event = v1::ScheduleEvent {
            event: Some(created.into()),
        };

        let ScheduleChange::Created {
            schedule_id,
            definition,
        } = decode_schedule_change(&event).unwrap()
        else {
            panic!("expected Created");
        };
        assert_eq!(schedule_id.as_str(), "orders/created");
        assert_eq!(definition.status, ScheduleEventStatus::Paused);
    }

    #[test]
    fn recorded_events_decode_for_pause_resume_remove() {
        for (event, expect_id) in [
            (
                v1::ScheduleEvent {
                    event: Some(
                        v1::SchedulePaused {
                            schedule_id: "a".to_string(),
                        }
                        .into(),
                    ),
                },
                "a",
            ),
            (
                v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleResumed {
                            schedule_id: "b".to_string(),
                        }
                        .into(),
                    ),
                },
                "b",
            ),
            (
                v1::ScheduleEvent {
                    event: Some(
                        v1::ScheduleRemoved {
                            schedule_id: "c".to_string(),
                        }
                        .into(),
                    ),
                },
                "c",
            ),
        ] {
            let change = decode_schedule_change(&event).unwrap();
            assert_eq!(change.schedule_id().as_str(), expect_id);
        }
    }

    #[test]
    fn missing_event_case_is_an_error() {
        let event = v1::ScheduleEvent { event: None };
        assert!(matches!(
            decode_schedule_change(&event).unwrap_err(),
            ScheduleEventDecodeError::MissingEvent
        ));
    }

    #[test]
    fn occurrence_lifecycle_events_decode_into_schedule_changes() {
        use trogon_decider_runtime::{Event, EventEncode, EventId, EventType, Headers, StreamEvent, StreamPosition};
        use uuid::Uuid;

        let occurrence_at = at_instant();
        let event = v1::ScheduleEvent {
            event: Some(
                v1::ScheduleOccurrenceRecorded {
                    schedule_id: "backup".to_string(),
                    occurrence_sequence: Some(2),
                    occurrence_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at)),
                    recorded_at: MessageField::some(trogonai_proto::convert::timestamp_from_datetime(&occurrence_at)),
                }
                .into(),
            ),
        };

        let change = decode_schedule_change(&event).unwrap();
        assert!(matches!(
            change,
            ScheduleChange::OccurrenceRecorded {
                ref schedule_id,
                ref occurrence_sequence,
                occurrence_at: decoded_at,
            } if schedule_id.as_str() == "backup"
                && occurrence_sequence.as_u64() == 2
                && decoded_at == occurrence_at
        ));

        let stream_event = StreamEvent {
            stream_id: "backup".to_string(),
            event: Event {
                id: EventId::new(Uuid::from_u128(3)),
                r#type: event.event_type().expect("event has a type").to_string(),
                content: EventEncode::encode(&event).expect("event encodes"),
                headers: Headers::empty(),
            },
            stream_position: StreamPosition::try_new(1).expect("position is non-zero"),
            recorded_at: at_instant(),
        };

        assert!(matches!(
            schedule_change_from_stream_event(&stream_event).unwrap(),
            Some(ScheduleChange::OccurrenceRecorded { .. })
        ));
    }

    #[test]
    fn negative_duration_is_rejected() {
        let error = proto_duration_to_std(&ProtoDuration::from_secs_nanos(-1, 0), "every").unwrap_err();
        assert!(matches!(error, ScheduleEventDecodeError::Duration { field: "every" }));
    }

    #[test]
    fn out_of_range_duration_nanos_is_rejected() {
        let mut duration = ProtoDuration::from_secs_nanos(1, 0);
        duration.nanos = 1_000_000_000;
        let error = proto_duration_to_std(&duration, "every").unwrap_err();
        assert!(matches!(error, ScheduleEventDecodeError::Duration { field: "every" }));
    }

    #[test]
    fn lane_key_routes_decodable_events_by_payload_schedule_id() {
        use trogon_decider_runtime::{Event, EventEncode, EventId, EventType, Headers, StreamEvent, StreamPosition};
        use uuid::Uuid;

        let created = v1::ScheduleCreated {
            schedule_id: "orders/created".to_string(),
            status: MessageField::some(v1::ScheduleStatus::from(ScheduleEventStatus::Scheduled)),
            schedule: MessageField::some(proto_schedule(&DomainSchedule::every(Duration::from_secs(30)).unwrap())),
            delivery: MessageField::some(proto_delivery(&DomainDelivery::nats_event("agent.run").unwrap())),
            message: MessageField::some(proto_message(&DomainMessage {
                content: DomainContent::json("{}"),
                headers: DomainHeaders::default(),
            })),
        };
        let event = v1::ScheduleEvent {
            event: Some(created.into()),
        };
        let content = EventEncode::encode(&event).expect("schedule event encodes");
        let r#type = event.event_type().expect("schedule event has a type").to_string();
        let stream_event = StreamEvent {
            stream_id: "wrong-stream".to_string(),
            event: Event {
                id: EventId::new(Uuid::from_u128(1)),
                r#type,
                content,
                headers: Headers::empty(),
            },
            stream_position: StreamPosition::try_new(1).expect("position is non-zero"),
            recorded_at: at_instant(),
        };

        let (key, decoded) = lane_route_from_stream_event(&stream_event);
        assert_eq!(key, ScheduleKey::derive(&ScheduleId::parse("orders/created").unwrap()));
        assert_ne!(key, ScheduleKey::for_stream(&StreamRoutingId::from("wrong-stream")));
        // The decoded change is threaded so the worker does not decode again.
        let DecodedScheduleEvent::Change(change) = decoded else {
            panic!("expected the decoded schedule change to be threaded");
        };
        assert_eq!(change.schedule_id().as_str(), "orders/created");
    }

    #[test]
    fn lane_key_falls_back_to_stream_id_for_foreign_events() {
        use trogon_decider_runtime::{Event, EventId, Headers, StreamEvent, StreamPosition};
        use uuid::Uuid;

        let stream_event = StreamEvent {
            stream_id: "orders/created".to_string(),
            event: Event {
                id: EventId::new(Uuid::from_u128(2)),
                r#type: "foreign.event.v1".to_string(),
                content: b"{}".to_vec(),
                headers: Headers::empty(),
            },
            stream_position: StreamPosition::try_new(1).expect("position is non-zero"),
            recorded_at: at_instant(),
        };

        let (key, decoded) = lane_route_from_stream_event(&stream_event);
        assert_eq!(
            key,
            ScheduleKey::for_stream(&StreamRoutingId::from(stream_event.stream_id.as_str()))
        );
        assert!(matches!(decoded, DecodedScheduleEvent::Foreign));
    }
}
