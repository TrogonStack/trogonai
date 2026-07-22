use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};

use crate::commands::domain::{
    Delivery, DeliveryRoute, HeaderName, HeaderValue, SamplingSource, Schedule, ScheduleId, ScheduleMessage,
    ScheduleOccurrenceSequence,
};

use super::RRuleExpansionError;
use super::{
    GoDurationError, RRuleWakeupPayload, RRuleWakeupPayloadEncodeError, ScheduleKey, ScheduleSubject,
    format_go_duration,
};

const NATS_SCHEDULE_HEADER: &str = "Nats-Schedule";
const NATS_SCHEDULE_TIME_ZONE_HEADER: &str = "Nats-Schedule-Time-Zone";
const NATS_SCHEDULE_TARGET_HEADER: &str = "Nats-Schedule-Target";
const NATS_SCHEDULE_TTL_HEADER: &str = "Nats-Schedule-TTL";
const NATS_SCHEDULE_SOURCE_HEADER: &str = "Nats-Schedule-Source";
const CONTENT_TYPE_HEADER: &str = "Content-Type";
const TROGON_SCHEDULE_KEY_HEADER: &str = "Trogon-Schedule-Key";
const TROGON_SCHEDULE_ID_B64_HEADER: &str = "Trogon-Schedule-Id-B64";
const TROGON_SCHEDULE_OCCURRENCE_SEQUENCE_HEADER: &str = "Trogon-Schedule-Occurrence-Sequence";
const TROGON_SCHEDULE_OCCURRENCE_AT_HEADER: &str = "Trogon-Schedule-Occurrence-At";
const TROGON_SCHEDULE_RESERVED_PREFIX: &str = "Trogon-Schedule";

const NATS_RESERVED_HEADERS: [&str; 6] = [
    "Nats-Msg-Id",
    "Nats-Schedule",
    "Nats-Schedule-Source",
    "Nats-Schedule-Target",
    "Nats-Schedule-Time-Zone",
    "Nats-Schedule-TTL",
];

#[derive(Debug, thiserror::Error)]
pub enum ScheduleRequestError {
    #[error("schedule kind is not supported by NATS message scheduling")]
    UnsupportedSchedule,
    #[error("RRULE schedule expansion failed: {source}")]
    RRuleExpansion {
        #[source]
        source: RRuleExpansionError,
    },
    #[error("schedule definition could not be encoded: {source}")]
    ScheduleEncoding {
        #[source]
        source: trogonai_proto::convert::DurationConversionError,
    },
    #[error("{field} duration is invalid: {source}")]
    GoDuration {
        field: &'static str,
        #[source]
        source: GoDurationError,
    },
    #[error("user header '{name}' is scheduler-owned")]
    ReservedUserHeader { name: String },
    #[error("delivery target '{subject}' is inside a scheduler-owned namespace")]
    TargetIsSchedulerInternal { subject: String },
    #[error("delivery source sampling is not supported for scheduler-dispatched recurrence")]
    UnsupportedDispatchSource,
    #[error("scheduler header '{name}' has a value with characters invalid for NATS headers")]
    InvalidHeaderValue { name: &'static str },
    #[error("scheduler header '{name}' has a name with characters invalid for NATS headers")]
    InvalidHeaderName { name: &'static str },
    #[error("RRULE wakeup payload could not be encoded: {source}")]
    RRuleWakeupPayloadEncode {
        #[source]
        source: RRuleWakeupPayloadEncodeError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleHeader {
    name: HeaderName,
    value: HeaderValue,
}

impl ScheduleHeader {
    pub fn name(&self) -> &HeaderName {
        &self.name
    }

    pub fn value(&self) -> &HeaderValue {
        &self.value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRequest {
    subject: ScheduleSubject,
    headers: Vec<ScheduleHeader>,
    payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchRequest {
    subject: DeliveryRoute,
    headers: Vec<ScheduleHeader>,
    payload: Vec<u8>,
}

impl ScheduleRequest {
    pub fn build(
        schedule_id: &ScheduleId,
        schedule: &Schedule,
        delivery: &Delivery,
        message: &ScheduleMessage,
    ) -> Result<Self, ScheduleRequestError> {
        let schedule_value = schedule_header_value(schedule)?;
        let timezone = match schedule {
            Schedule::Cron {
                timezone: Some(timezone),
                ..
            } => Some(timezone.as_str()),
            _ => None,
        };

        Self::build_with_schedule_value(schedule_id, schedule_value, timezone, delivery, message)
    }

    pub fn build_at(
        schedule_id: &ScheduleId,
        at: DateTime<Utc>,
        delivery: &Delivery,
        message: &ScheduleMessage,
    ) -> Result<Self, ScheduleRequestError> {
        Self::build_with_schedule_value(schedule_id, at_schedule_value(at), None, delivery, message)
    }

    pub fn build_rrule_wakeup(
        schedule_id: &ScheduleId,
        at: DateTime<Utc>,
        delivery: &Delivery,
    ) -> Result<Self, ScheduleRequestError> {
        let key = ScheduleKey::derive(schedule_id);
        let subject = ScheduleSubject::execution(&key);
        let target = ScheduleSubject::rrule_wakeup(&key);
        let payload = RRuleWakeupPayload::new(schedule_id.clone(), at)
            .encode()
            .map_err(|source| ScheduleRequestError::RRuleWakeupPayloadEncode { source })?;

        let mut headers = Vec::new();
        push_header(&mut headers, NATS_SCHEDULE_HEADER, at_schedule_value(at))?;
        push_header(&mut headers, NATS_SCHEDULE_TARGET_HEADER, target.as_str().to_string())?;
        let Delivery::NatsEvent { ttl, .. } = delivery;
        if let Some(ttl) = ttl {
            let formatted = format_go_duration(ttl.as_duration())
                .map_err(|source| ScheduleRequestError::GoDuration { field: "ttl", source })?;
            push_header(&mut headers, NATS_SCHEDULE_TTL_HEADER, formatted)?;
        }
        push_header(&mut headers, CONTENT_TYPE_HEADER, "application/json")?;
        push_header(&mut headers, TROGON_SCHEDULE_KEY_HEADER, key.simple())?;
        let schedule_id_b64 = URL_SAFE_NO_PAD.encode(schedule_id.as_str());
        push_header(&mut headers, TROGON_SCHEDULE_ID_B64_HEADER, schedule_id_b64)?;

        Ok(Self {
            subject,
            headers,
            payload,
        })
    }

    fn build_with_schedule_value(
        schedule_id: &ScheduleId,
        schedule_value: String,
        timezone: Option<&str>,
        delivery: &Delivery,
        message: &ScheduleMessage,
    ) -> Result<Self, ScheduleRequestError> {
        let key = ScheduleKey::derive(schedule_id);
        let subject = ScheduleSubject::execution(&key);

        let Delivery::NatsEvent { route, ttl, source } = delivery;
        if ScheduleSubject::is_scheduler_internal(route.as_str()) {
            return Err(ScheduleRequestError::TargetIsSchedulerInternal {
                subject: route.as_str().to_string(),
            });
        }

        let mut headers = Vec::new();
        push_header(&mut headers, NATS_SCHEDULE_HEADER, schedule_value)?;
        if let Some(timezone) = timezone {
            push_header(&mut headers, NATS_SCHEDULE_TIME_ZONE_HEADER, timezone.to_string())?;
        }
        push_header(&mut headers, NATS_SCHEDULE_TARGET_HEADER, route.as_str().to_string())?;
        if let Some(ttl) = ttl {
            let formatted = format_go_duration(ttl.as_duration())
                .map_err(|source| ScheduleRequestError::GoDuration { field: "ttl", source })?;
            push_header(&mut headers, NATS_SCHEDULE_TTL_HEADER, formatted)?;
        }
        if let Some(SamplingSource::LatestFromSubject { subject: sampling }) = source {
            push_header(&mut headers, NATS_SCHEDULE_SOURCE_HEADER, sampling.as_str().to_string())?;
        }
        let content_type = message.content.content_type().as_str().to_string();
        push_header(&mut headers, CONTENT_TYPE_HEADER, content_type)?;
        push_header(&mut headers, TROGON_SCHEDULE_KEY_HEADER, key.simple())?;
        let schedule_id_b64 = URL_SAFE_NO_PAD.encode(schedule_id.as_str());
        push_header(&mut headers, TROGON_SCHEDULE_ID_B64_HEADER, schedule_id_b64)?;

        for header in message.headers.as_slice() {
            let name = header.name().as_str();
            if is_reserved_header(name) {
                return Err(ScheduleRequestError::ReservedUserHeader { name: name.to_string() });
            }
            headers.push(ScheduleHeader {
                name: header.name().clone(),
                value: header.value().clone(),
            });
        }

        Ok(Self {
            subject,
            headers,
            payload: message.content.as_slice().to_vec(),
        })
    }

    pub fn subject(&self) -> &ScheduleSubject {
        &self.subject
    }

    pub fn headers(&self) -> &[ScheduleHeader] {
        &self.headers
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

impl DispatchRequest {
    pub fn build(
        schedule_id: &ScheduleId,
        delivery: &Delivery,
        message: &ScheduleMessage,
    ) -> Result<Self, ScheduleRequestError> {
        Self::build_with_occurrence(schedule_id, None, delivery, message)
    }

    pub fn build_occurrence(
        schedule_id: &ScheduleId,
        occurrence_sequence: ScheduleOccurrenceSequence,
        occurrence_at: DateTime<Utc>,
        delivery: &Delivery,
        message: &ScheduleMessage,
    ) -> Result<Self, ScheduleRequestError> {
        Self::build_with_occurrence(
            schedule_id,
            Some((occurrence_sequence, occurrence_at)),
            delivery,
            message,
        )
    }

    fn build_with_occurrence(
        schedule_id: &ScheduleId,
        occurrence: Option<(ScheduleOccurrenceSequence, DateTime<Utc>)>,
        delivery: &Delivery,
        message: &ScheduleMessage,
    ) -> Result<Self, ScheduleRequestError> {
        let key = ScheduleKey::derive(schedule_id);
        let Delivery::NatsEvent { route, source, .. } = delivery;
        if ScheduleSubject::is_scheduler_internal(route.as_str()) {
            return Err(ScheduleRequestError::TargetIsSchedulerInternal {
                subject: route.as_str().to_string(),
            });
        }
        if source.is_some() {
            return Err(ScheduleRequestError::UnsupportedDispatchSource);
        }

        let mut headers = Vec::new();
        let content_type = message.content.content_type().as_str().to_string();
        push_header(&mut headers, CONTENT_TYPE_HEADER, content_type)?;
        push_header(&mut headers, TROGON_SCHEDULE_KEY_HEADER, key.simple())?;
        let schedule_id_b64 = URL_SAFE_NO_PAD.encode(schedule_id.as_str());
        push_header(&mut headers, TROGON_SCHEDULE_ID_B64_HEADER, schedule_id_b64)?;
        if let Some((sequence, occurrence_at)) = occurrence {
            push_header(
                &mut headers,
                TROGON_SCHEDULE_OCCURRENCE_SEQUENCE_HEADER,
                sequence.as_u64().to_string(),
            )?;
            push_header(
                &mut headers,
                TROGON_SCHEDULE_OCCURRENCE_AT_HEADER,
                occurrence_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
            )?;
        }
        for header in message.headers.as_slice() {
            let name = header.name().as_str();
            if is_reserved_header(name) {
                return Err(ScheduleRequestError::ReservedUserHeader { name: name.to_string() });
            }
            headers.push(ScheduleHeader {
                name: header.name().clone(),
                value: header.value().clone(),
            });
        }

        Ok(Self {
            subject: route.clone(),
            headers,
            payload: message.content.as_slice().to_vec(),
        })
    }

    pub fn subject(&self) -> &DeliveryRoute {
        &self.subject
    }

    pub fn headers(&self) -> &[ScheduleHeader] {
        &self.headers
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

fn schedule_header_value(schedule: &Schedule) -> Result<String, ScheduleRequestError> {
    match schedule {
        Schedule::At { at } => Ok(at_schedule_value(*at)),
        Schedule::Every { every } => {
            let formatted = format_go_duration(every.as_duration())
                .map_err(|source| ScheduleRequestError::GoDuration { field: "every", source })?;
            Ok(format!("@every {formatted}"))
        }
        Schedule::Cron { expr, .. } => Ok(expr.as_str().to_string()),
        Schedule::RRule { .. } => Err(ScheduleRequestError::UnsupportedSchedule),
    }
}

fn at_schedule_value(at: DateTime<Utc>) -> String {
    format!("@at {}", at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true))
}

/// Header names are static scheduler-owned literals; values can derive from
/// user-supplied data (route, timezone, cron expression, sampling subject), so
/// value validation failures surface as errors instead of panics.
fn push_header(
    headers: &mut Vec<ScheduleHeader>,
    name: &'static str,
    value: impl Into<String>,
) -> Result<(), ScheduleRequestError> {
    let header_name = HeaderName::new(name).map_err(|_| ScheduleRequestError::InvalidHeaderName { name })?;
    let value = HeaderValue::new(value).map_err(|_| ScheduleRequestError::InvalidHeaderValue { name })?;
    headers.push(ScheduleHeader {
        name: header_name,
        value,
    });
    Ok(())
}

fn is_reserved_header(name: &str) -> bool {
    if NATS_RESERVED_HEADERS
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(name))
    {
        return true;
    }
    if CONTENT_TYPE_HEADER.eq_ignore_ascii_case(name) {
        return true;
    }
    matches!(
        name.get(..TROGON_SCHEDULE_RESERVED_PREFIX.len()),
        Some(prefix) if prefix.eq_ignore_ascii_case(TROGON_SCHEDULE_RESERVED_PREFIX)
    )
}

#[cfg(test)]
mod tests;
