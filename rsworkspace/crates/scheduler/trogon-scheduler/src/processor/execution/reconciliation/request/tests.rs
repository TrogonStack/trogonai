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
