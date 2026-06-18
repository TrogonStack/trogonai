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
mod tests {
    use std::time::Duration;

    use chrono::{DateTime, Utc};

    use super::*;
    use crate::commands::domain::{MessageContent, ScheduleHeaders, TtlDuration};

    fn schedule_id(raw: &str) -> ScheduleId {
        ScheduleId::parse(raw).unwrap()
    }

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
    }

    fn message() -> ScheduleMessage {
        ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::default(),
        }
    }

    fn header_value<'a>(request: &'a ScheduleRequest, name: &str) -> Option<&'a str> {
        request
            .headers()
            .iter()
            .find(|header| header.name().as_str() == name)
            .map(|header| header.value().as_str())
    }

    fn dispatch_header_value<'a>(request: &'a DispatchRequest, name: &str) -> Option<&'a str> {
        request
            .headers()
            .iter()
            .find(|header| header.name().as_str() == name)
            .map(|header| header.value().as_str())
    }

    #[test]
    fn build_at_uses_one_shot_schedule_headers() {
        let id = schedule_id("orders/created");
        let request = ScheduleRequest::build_at(
            &id,
            at("2026-01-01T00:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();

        assert_eq!(
            header_value(&request, "Nats-Schedule"),
            Some("@at 2026-01-01T00:00:00Z")
        );
        assert_eq!(header_value(&request, "Nats-Schedule-Target"), Some("agent.run"));
        assert_eq!(
            request.subject().as_str(),
            ScheduleSubject::execution(&ScheduleKey::derive(&id)).as_str()
        );
    }

    #[test]
    fn at_schedule_maps_to_an_at_header_and_correlation_headers() {
        let id = schedule_id("orders/created");
        let request = ScheduleRequest::build(
            &id,
            &Schedule::At {
                at: at("2026-01-01T00:00:00Z"),
            },
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();

        assert_eq!(
            header_value(&request, "Nats-Schedule"),
            Some("@at 2026-01-01T00:00:00Z")
        );
        assert_eq!(header_value(&request, "Nats-Schedule-Target"), Some("agent.run"));
        assert_eq!(header_value(&request, "Content-Type"), Some("application/json"));
        assert_eq!(
            header_value(&request, "Trogon-Schedule-Key"),
            Some(ScheduleKey::derive(&id).simple().as_str())
        );
        assert_eq!(
            header_value(&request, "Trogon-Schedule-Id-B64"),
            Some("b3JkZXJzL2NyZWF0ZWQ")
        );
        assert_eq!(request.payload(), br#"{"ok":true}"#);
        assert_eq!(
            request.subject().as_str(),
            ScheduleSubject::execution(&ScheduleKey::derive(&id)).as_str()
        );
    }

    #[test]
    fn at_schedule_preserves_subsecond_precision() {
        let request = ScheduleRequest::build(
            &schedule_id("orders/created"),
            &Schedule::At {
                at: at("2026-01-01T00:00:00.123456789Z"),
            },
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();

        assert_eq!(
            header_value(&request, "Nats-Schedule"),
            Some("@at 2026-01-01T00:00:00.123456789Z")
        );
    }

    #[test]
    fn every_schedule_formats_a_go_duration() {
        let request = ScheduleRequest::build(
            &schedule_id("heartbeat"),
            &Schedule::every(Duration::from_secs(90)).unwrap(),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();

        assert_eq!(header_value(&request, "Nats-Schedule"), Some("@every 1m30s"));
    }

    #[test]
    fn cron_schedule_emits_expression_and_optional_timezone() {
        let request = ScheduleRequest::build(
            &schedule_id("nightly"),
            &Schedule::cron("0 0 * * * *", Some("America/New_York".to_string())).unwrap(),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();

        assert_eq!(header_value(&request, "Nats-Schedule"), Some("0 0 * * * *"));
        assert_eq!(
            header_value(&request, "Nats-Schedule-Time-Zone"),
            Some("America/New_York")
        );
    }

    #[test]
    fn delivery_ttl_and_source_become_headers() {
        let delivery = Delivery::NatsEvent {
            route: crate::commands::domain::DeliveryRoute::new("agent.run").unwrap(),
            ttl: Some(crate::commands::domain::TtlDuration::from_secs(60).unwrap()),
            source: Some(SamplingSource::latest_from_subject("agent.events").unwrap()),
        };
        let request = ScheduleRequest::build(
            &schedule_id("sampled"),
            &Schedule::every(Duration::from_secs(30)).unwrap(),
            &delivery,
            &message(),
        )
        .unwrap();

        assert_eq!(header_value(&request, "Nats-Schedule-TTL"), Some("1m"));
        assert_eq!(header_value(&request, "Nats-Schedule-Source"), Some("agent.events"));
    }

    #[test]
    fn rrule_schedule_is_unsupported() {
        let error = ScheduleRequest::build(
            &schedule_id("recurring"),
            &Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap_err();

        assert!(matches!(error, ScheduleRequestError::UnsupportedSchedule));
    }

    #[test]
    fn rrule_wakeup_targets_scheduler_owned_subject_and_carries_ttl() {
        let id = schedule_id("orders/recurring");
        let delivery = Delivery::NatsEvent {
            route: DeliveryRoute::new("agent.run").unwrap(),
            ttl: Some(TtlDuration::from_secs(45).unwrap()),
            source: None,
        };
        let request = ScheduleRequest::build_rrule_wakeup(&id, at("2026-06-15T18:00:00Z"), &delivery).unwrap();
        let key = ScheduleKey::derive(&id);

        assert_eq!(request.subject().as_str(), ScheduleSubject::execution(&key).as_str());
        assert_eq!(
            header_value(&request, "Nats-Schedule"),
            Some("@at 2026-06-15T18:00:00Z")
        );
        assert_eq!(
            header_value(&request, "Nats-Schedule-Target"),
            Some(ScheduleSubject::rrule_wakeup(&key).as_str())
        );
        assert_eq!(header_value(&request, "Nats-Schedule-TTL"), Some("45s"));
        assert_eq!(header_value(&request, "Content-Type"), Some("application/json"));
        assert_eq!(
            request.payload(),
            br#"{"schedule_id":"orders/recurring","occurrence_at":"2026-06-15T18:00:00Z"}"#
        );
    }

    #[test]
    fn dispatch_request_targets_user_subject_without_schedule_headers() {
        let message = ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        };
        let request = DispatchRequest::build(
            &schedule_id("orders/created"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message,
        )
        .unwrap();

        assert_eq!(request.subject().as_str(), "agent.run");
        assert_eq!(
            dispatch_header_value(&request, "Content-Type"),
            Some("application/json")
        );
        assert_eq!(
            dispatch_header_value(&request, "Trogon-Schedule-Key"),
            Some(ScheduleKey::derive(&schedule_id("orders/created")).simple().as_str())
        );
        assert_eq!(
            dispatch_header_value(&request, "Trogon-Schedule-Id-B64"),
            Some("b3JkZXJzL2NyZWF0ZWQ")
        );
        assert_eq!(
            dispatch_header_value(&request, "Trogon-Schedule-Occurrence-Sequence"),
            None
        );
        assert_eq!(dispatch_header_value(&request, "x-kind"), Some("heartbeat"));
        assert_eq!(request.payload(), br#"{"ok":true}"#);
    }

    #[test]
    fn occurrence_dispatch_request_carries_read_model_identity_headers() {
        let request = DispatchRequest::build_occurrence(
            &schedule_id("orders/created"),
            ScheduleOccurrenceSequence::try_new(12).unwrap(),
            at("2026-06-15T18:00:00Z"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message(),
        )
        .unwrap();

        assert_eq!(
            dispatch_header_value(&request, "Trogon-Schedule-Occurrence-Sequence"),
            Some("12")
        );
        assert_eq!(
            dispatch_header_value(&request, "Trogon-Schedule-Occurrence-At"),
            Some("2026-06-15T18:00:00Z")
        );
    }

    #[test]
    fn dispatch_request_rejects_sampling_and_scheduler_owned_headers() {
        let delivery = Delivery::NatsEvent {
            route: DeliveryRoute::new("agent.run").unwrap(),
            ttl: None,
            source: Some(SamplingSource::latest_from_subject("agent.events").unwrap()),
        };
        let sampling = DispatchRequest::build(&schedule_id("orders/created"), &delivery, &message()).unwrap_err();
        assert!(matches!(sampling, ScheduleRequestError::UnsupportedDispatchSource));

        let message = ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::new([("Nats-Msg-Id", "value")]).unwrap(),
        };
        let reserved = DispatchRequest::build(
            &schedule_id("orders/created"),
            &Delivery::nats_event("agent.run").unwrap(),
            &message,
        )
        .unwrap_err();
        assert!(matches!(reserved, ScheduleRequestError::ReservedUserHeader { .. }));
    }

    #[test]
    fn nats_reserved_headers_are_detected() {
        for reserved in [
            "Nats-Msg-Id",
            "nats-msg-id",
            "Nats-Schedule",
            "Nats-Schedule-Source",
            "Nats-Schedule-Target",
            "Nats-Schedule-Time-Zone",
            "Nats-Schedule-TTL",
        ] {
            assert!(is_reserved_header(reserved), "{reserved}");
        }
    }

    #[test]
    fn reserved_user_headers_are_rejected() {
        for reserved in [
            "Content-Type",
            "content-type",
            "Nats-Msg-Id",
            "Trogon-Schedule-Key",
            "trogon-schedule-extra",
        ] {
            let with_reserved = ScheduleMessage {
                content: MessageContent::json("{}"),
                headers: ScheduleHeaders::new([(reserved, "value")]).unwrap(),
            };
            let error = ScheduleRequest::build(
                &schedule_id("orders"),
                &Schedule::At {
                    at: at("2026-01-01T00:00:00Z"),
                },
                &Delivery::nats_event("agent.run").unwrap(),
                &with_reserved,
            )
            .unwrap_err();

            assert!(
                matches!(error, ScheduleRequestError::ReservedUserHeader { .. }),
                "{reserved}"
            );
        }
    }

    #[test]
    fn user_headers_are_copied_through_after_the_scheduler_headers() {
        let with_user = ScheduleMessage {
            content: MessageContent::json("{}"),
            headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        };
        let request = ScheduleRequest::build(
            &schedule_id("orders"),
            &Schedule::At {
                at: at("2026-01-01T00:00:00Z"),
            },
            &Delivery::nats_event("agent.run").unwrap(),
            &with_user,
        )
        .unwrap();

        assert_eq!(header_value(&request, "x-kind"), Some("heartbeat"));
    }

    #[test]
    fn delivery_target_inside_a_scheduler_namespace_is_rejected() {
        let id = schedule_id("orders");
        let own_subject = ScheduleSubject::execution(&ScheduleKey::derive(&id));
        let other_subject = ScheduleSubject::execution(&ScheduleKey::derive(&schedule_id("other")));
        let event_subject = ScheduleSubject::event(&ScheduleKey::derive(&id));

        for target in [
            own_subject.as_str(),
            other_subject.as_str(),
            event_subject.as_str(),
            "trogon.scheduler.corrupt-checkpoint",
            "trogon.scheduler.anything",
        ] {
            let error = ScheduleRequest::build(
                &id,
                &Schedule::At {
                    at: at("2026-01-01T00:00:00Z"),
                },
                &Delivery::nats_event(target).unwrap(),
                &message(),
            )
            .unwrap_err();

            assert!(
                matches!(error, ScheduleRequestError::TargetIsSchedulerInternal { .. }),
                "{target}"
            );
        }
    }

    #[test]
    fn delivery_target_outside_scheduler_namespaces_is_accepted() {
        for target in [
            "scheduler.schedules.execution.v2.key",
            "scheduler.other",
            "trogonscheduler.run",
            "agent.run",
        ] {
            assert!(!ScheduleSubject::is_scheduler_internal(target), "{target}");
        }
    }

    #[test]
    fn request_errors_display_and_expose_sources() {
        let unsupported = ScheduleRequestError::UnsupportedSchedule;
        assert_eq!(
            unsupported.to_string(),
            "schedule kind is not supported by NATS message scheduling"
        );
        assert!(std::error::Error::source(&unsupported).is_none());

        let go_duration = ScheduleRequestError::GoDuration {
            field: "every",
            source: GoDurationError::TooLarge {
                max_nanos: 1,
                actual_nanos: 2,
            },
        };
        assert_eq!(
            go_duration.to_string(),
            "every duration is invalid: duration of 2ns exceeds the maximum Go duration of 1ns"
        );
        assert!(std::error::Error::source(&go_duration).is_some());

        let reserved = ScheduleRequestError::ReservedUserHeader {
            name: "Content-Type".to_string(),
        };
        assert_eq!(reserved.to_string(), "user header 'Content-Type' is scheduler-owned");
        assert!(std::error::Error::source(&reserved).is_none());

        let target = ScheduleRequestError::TargetIsSchedulerInternal {
            subject: "scheduler.schedules.execution.v1.key".to_string(),
        };
        assert_eq!(
            target.to_string(),
            "delivery target 'scheduler.schedules.execution.v1.key' is inside a scheduler-owned namespace"
        );
        assert!(std::error::Error::source(&target).is_none());
    }

    #[test]
    fn base64url_encodes_without_padding() {
        assert_eq!(URL_SAFE_NO_PAD.encode(""), "");
        assert_eq!(URL_SAFE_NO_PAD.encode("f"), "Zg");
        assert_eq!(URL_SAFE_NO_PAD.encode("fo"), "Zm8");
        assert_eq!(URL_SAFE_NO_PAD.encode("foo"), "Zm9v");
        assert_eq!(URL_SAFE_NO_PAD.encode("orders/created"), "b3JkZXJzL2NyZWF0ZWQ");
    }
}
