//! Decoding the stored `projections.v1.ScheduleProjection` protobuf into the
//! read-model value objects that queries return. This is the read side of the
//! projection: the projection writes the proto, the queries read and shape it for
//! callers.
//!
//! Under the coverage build the query entry points that call this are stubbed out,
//! so the decode path is reached only by its own tests; allow it to look unused
//! there.
#![cfg_attr(coverage, allow(dead_code))]

use chrono::{TimeZone, Utc};

use crate::{error::SchedulerError, projections_v1};

use projections_v1::__buffa::oneof::delivery::Kind as DeliveryKind;
use projections_v1::__buffa::oneof::delivery::nats_message::source::Kind as SourceKind;
use projections_v1::__buffa::oneof::schedule::Kind as ScheduleKind;
use projections_v1::__buffa::oneof::schedule_status::Kind as ScheduleStatusKind;

use super::read_model::{
    MessageContent, MessageEnvelope, MessageHeaders, Schedule, ScheduleEventDelivery, ScheduleEventSamplingSource,
    ScheduleEventSchedule, ScheduleEventStatus,
};

/// Decodes a stored KV value into the read-model schedule callers see.
pub(crate) fn decode_schedule(value: &[u8]) -> Result<Schedule, SchedulerError> {
    let view = <projections_v1::ScheduleProjection as buffa::Message>::decode_from_slice(value).map_err(|source| {
        SchedulerError::kv_source(
            "failed to decode projected schedule view",
            std::io::Error::other(source.to_string()),
        )
    })?;
    schedule_from_view(&view)
}

fn malformed(context: &'static str) -> SchedulerError {
    SchedulerError::kv_source("projected schedule view is malformed", std::io::Error::other(context))
}

pub(crate) fn schedule_from_view(view: &projections_v1::ScheduleProjection) -> Result<Schedule, SchedulerError> {
    let schedule = view.schedule.as_option().ok_or_else(|| malformed("missing schedule"))?;
    let delivery = view.delivery.as_option().ok_or_else(|| malformed("missing delivery"))?;
    let message = view.message.as_option().ok_or_else(|| malformed("missing message"))?;

    Ok(Schedule {
        id: view.schedule_id.clone(),
        status: status_from_proto(view.status.as_option()),
        completed: view.completed.unwrap_or(false),
        next_occurrence_at: view.next_occurrence_at.as_option().map(timestamp_to_datetime),
        last_occurrence_at: view.last_occurrence_at.as_option().map(timestamp_to_datetime),
        schedule: schedule_from_proto(schedule)?,
        delivery: delivery_from_proto(delivery)?,
        message: message_from_proto(message)?,
    })
}

fn status_from_proto(status: Option<&projections_v1::ScheduleStatus>) -> ScheduleEventStatus {
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

fn duration_from_proto(duration: &buffa_types::google::protobuf::Duration) -> std::time::Duration {
    // Schedule intervals and TTLs are non-negative; clamp defensively so a stray
    // negative component cannot panic `Duration::new`.
    std::time::Duration::new(duration.seconds.max(0) as u64, duration.nanos.max(0) as u32)
}

fn schedule_from_proto(schedule: &projections_v1::Schedule) -> Result<ScheduleEventSchedule, SchedulerError> {
    match schedule.kind.as_ref() {
        Some(ScheduleKind::At(inner)) => Ok(ScheduleEventSchedule::At {
            at: inner.at.as_option().map(timestamp_to_datetime).unwrap_or_default(),
        }),
        Some(ScheduleKind::Every(inner)) => Ok(ScheduleEventSchedule::Every {
            every: inner.every.as_option().map(duration_from_proto).unwrap_or_default(),
        }),
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
        None => Err(malformed("schedule has no supported case")),
    }
}

fn delivery_from_proto(delivery: &projections_v1::Delivery) -> Result<ScheduleEventDelivery, SchedulerError> {
    match delivery.kind.as_ref() {
        Some(DeliveryKind::NatsMessage(inner)) => Ok(ScheduleEventDelivery::NatsMessage {
            subject: inner.subject.clone(),
            ttl: inner.ttl.as_option().map(duration_from_proto),
            source: inner.source.as_option().map(sampling_source_from_proto).transpose()?,
        }),
        None => Err(malformed("delivery has no supported case")),
    }
}

fn sampling_source_from_proto(
    source: &projections_v1::delivery::nats_message::Source,
) -> Result<ScheduleEventSamplingSource, SchedulerError> {
    match source.kind.as_ref() {
        Some(SourceKind::LatestFromSubject(inner)) => Ok(ScheduleEventSamplingSource::LatestFromSubject {
            subject: inner.subject.clone(),
        }),
        None => Err(malformed("sampling source has no supported case")),
    }
}

fn message_from_proto(message: &projections_v1::Message) -> Result<MessageEnvelope, SchedulerError> {
    let content = match message.content.as_option() {
        Some(content) => {
            // The scheduler treats content as UTF-8 text and the executor rejects
            // non-UTF-8, so reject it here too rather than surfacing corruption.
            let body =
                String::from_utf8(content.data.clone()).map_err(|_| malformed("message content is not valid UTF-8"))?;
            MessageContent::new(content.content_type.clone(), body)
        }
        None => MessageContent::default(),
    };
    Ok(MessageEnvelope {
        content,
        headers: MessageHeaders::from_pairs(
            message
                .headers
                .iter()
                .map(|header| (header.name.clone(), header.value.clone())),
        ),
    })
}

#[cfg(test)]
mod tests;
