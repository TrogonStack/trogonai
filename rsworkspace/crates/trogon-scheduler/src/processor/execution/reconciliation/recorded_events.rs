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
            let occurrence_sequence = ScheduleOccurrenceSequence::try_new(scheduled.occurrence_sequence.ok_or(
                ScheduleEventDecodeError::MissingField {
                    field: "occurrence_sequence",
                },
            )?)
            .map_err(|source| ScheduleEventDecodeError::OccurrenceSequence { source })?;
            let occurrence_at = timestamp_to_datetime(
                scheduled
                    .occurrence_at
                    .as_option()
                    .ok_or(ScheduleEventDecodeError::MissingField { field: "occurrence_at" })?,
                "occurrence_at",
            )?;
            Ok(ScheduleChange::OccurrenceScheduled {
                schedule_id,
                occurrence_sequence,
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
mod tests;
