use std::error::Error;

use trogonai_proto::convert::PROTOBUF_DURATION_MAX_SECONDS;

use super::*;

#[test]
fn schedule_constructors_validate_and_preserve_values() {
    let cron = Schedule::cron("0 0 * * * *", Some("UTC".to_string())).unwrap();
    let rrule = Schedule::rrule(
        "2026-01-01T00:00:00Z",
        "RRULE:FREQ=DAILY;COUNT=2",
        Some("America/New_York".to_string()),
    )
    .unwrap();

    assert!(matches!(cron, Schedule::Cron { .. }));
    assert!(matches!(rrule, Schedule::RRule { .. }));
    assert!(Schedule::every(Duration::ZERO).is_err());
    assert!(Schedule::cron("not a cron", None).is_err());
    assert!(Schedule::rrule("tomorrow", "FREQ=DAILY;COUNT=2", None).is_err());
    assert!(
        Schedule::rrule(
            "2026-01-01T00:00:00Z",
            "FREQ=DAILY;COUNT=2",
            Some("Nope/Zone".to_string())
        )
        .is_err()
    );
}

#[test]
fn errors_are_scoped_to_the_value_that_failed_to_construct() {
    let every_zero = EveryDuration::new(Duration::ZERO).unwrap_err();
    assert_eq!(every_zero, EveryDurationError::MustBePositive);
    assert_eq!(every_zero.to_string(), "every duration must be positive");
    assert!(every_zero.source().is_none());

    let every_too_large = EveryDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).unwrap_err();
    assert_eq!(
        every_too_large,
        EveryDurationError::TooLarge {
            max: PROTOBUF_DURATION_MAX,
            actual: PROTOBUF_DURATION_MAX + Duration::from_nanos(1),
        }
    );
    assert_eq!(
        every_too_large.to_string(),
        format!(
            "every duration must be at most {max:?}, got {actual:?}",
            max = PROTOBUF_DURATION_MAX,
            actual = PROTOBUF_DURATION_MAX + Duration::from_nanos(1),
        )
    );
    assert!(every_too_large.source().is_none());

    let cron = CronExpression::new("not a cron").unwrap_err();
    assert!(matches!(cron, CronExpressionError::Invalid { ref expr, .. } if expr == "not a cron"));
    assert_eq!(
        cron.to_string(),
        format!("cron expression 'not a cron' is invalid: {}", cron.source().unwrap())
    );
    assert!(cron.source().is_some());

    let rrule_date = RRuleDateTime::new("dtstart", "tomorrow").unwrap_err();
    assert!(
        matches!(rrule_date, RRuleDateTimeError::Invalid { field, ref value, .. }
            if field == "dtstart" && value == "tomorrow")
    );
    assert_eq!(
        rrule_date.to_string(),
        format!(
            "dtstart datetime 'tomorrow' is invalid: {}",
            rrule_date.source().unwrap()
        )
    );
    assert!(rrule_date.source().is_some());

    let rrule = RRuleExpression::new("FREQDAILY").unwrap_err();
    assert_eq!(
        rrule.to_string(),
        "rrule 'FREQDAILY' is invalid: RRULE parts must use KEY=VALUE"
    );
    assert!(rrule.source().is_some());

    let timezone = TimeZone::new("Nope/Zone").unwrap_err();
    assert_eq!(
        timezone,
        TimeZoneError::Invalid {
            timezone: "Nope/Zone".to_string()
        }
    );
    assert_eq!(timezone.to_string(), "timezone 'Nope/Zone' is invalid");
    assert!(timezone.source().is_none());

    let tzdb_version = TzdbVersion::new("2025").unwrap_err();
    assert_eq!(
        tzdb_version,
        TzdbVersionError::Invalid {
            version: "2025".to_string()
        }
    );
    assert_eq!(tzdb_version.to_string(), "timezone database version '2025' is invalid");
    assert!(tzdb_version.source().is_none());

    let header = ScheduleHeaders::new([("bad name", "value")]).unwrap_err();
    assert_eq!(header.to_string(), "header name 'bad name' is invalid");
    assert!(header.source().is_some());

    let reserved_header = ScheduleHeaders::new([("Nats-Schedule", "value")]).unwrap_err();
    assert_eq!(
        reserved_header,
        ScheduleHeadersError::ReservedName {
            name: "Nats-Schedule".to_string()
        }
    );
    assert_eq!(reserved_header.to_string(), "header name 'Nats-Schedule' is reserved");
    assert!(reserved_header.source().is_none());

    let route = DeliveryRoute::new("bad*route").unwrap_err();
    assert!(matches!(route, DeliveryRouteError::Invalid { ref route, .. } if route == "bad*route"));
    assert_eq!(
        route.to_string(),
        format!("delivery route 'bad*route' is invalid: {}", route.source().unwrap())
    );
    assert!(route.source().is_some());

    let subject = SamplingSubject::new("bad>subject").unwrap_err();
    assert!(matches!(subject, SamplingSubjectError::Invalid { ref subject, .. } if subject == "bad>subject"));
    assert_eq!(
        subject.to_string(),
        format!(
            "sampling subject 'bad>subject' is invalid: {}",
            subject.source().unwrap()
        )
    );
    assert!(subject.source().is_some());

    let ttl_zero = TtlDuration::new(Duration::ZERO).unwrap_err();
    assert_eq!(ttl_zero, TtlDurationError::MustBePositive);
    assert_eq!(ttl_zero.to_string(), "ttl duration must be positive");
    assert!(ttl_zero.source().is_none());

    let ttl_too_large = TtlDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).unwrap_err();
    assert_eq!(
        ttl_too_large,
        TtlDurationError::TooLarge {
            max: PROTOBUF_DURATION_MAX,
            actual: PROTOBUF_DURATION_MAX + Duration::from_nanos(1),
        }
    );
    assert_eq!(
        ttl_too_large.to_string(),
        format!(
            "ttl duration must be at most {max:?}, got {actual:?}",
            max = PROTOBUF_DURATION_MAX,
            actual = PROTOBUF_DURATION_MAX + Duration::from_nanos(1),
        )
    );
    assert!(ttl_too_large.source().is_none());
}

#[test]
fn schedule_convenience_errors_only_wrap_schedule_value_failures() {
    let cron = Schedule::cron("not a cron", None).unwrap_err();
    assert!(matches!(cron, ScheduleError::CronExpression(_)));
    assert_eq!(
        cron.to_string(),
        CronExpression::new("not a cron").unwrap_err().to_string()
    );
    assert!(cron.source().is_some());

    let cron_timezone = Schedule::cron("0 0 * * * *", Some("Nope/Zone".to_string())).unwrap_err();
    assert!(matches!(cron_timezone, ScheduleError::TimeZone(_)));
    assert_eq!(cron_timezone.to_string(), "timezone 'Nope/Zone' is invalid");
    assert!(cron_timezone.source().is_some());

    let rrule_dtstart = Schedule::rrule("tomorrow", "FREQ=DAILY;COUNT=2", None).unwrap_err();
    assert!(matches!(rrule_dtstart, ScheduleError::RRuleDateTime(_)));
    assert_eq!(
        rrule_dtstart.to_string(),
        RRuleDateTime::new("dtstart", "tomorrow").unwrap_err().to_string()
    );
    assert!(rrule_dtstart.source().is_some());

    let rrule = Schedule::rrule("2026-01-01T00:00:00Z", "FREQDAILY", None).unwrap_err();
    assert!(matches!(rrule, ScheduleError::RRuleExpression(_)));
    assert_eq!(
        rrule.to_string(),
        "rrule 'FREQDAILY' is invalid: RRULE parts must use KEY=VALUE"
    );
    assert!(rrule.source().is_some());
}

#[test]
fn value_objects_cover_success_error_and_conversion_paths() {
    let every = EveryDuration::new(Duration::from_secs(30)).unwrap();
    let cron = CronExpression::try_from("0 0 * * * *".to_string()).unwrap();
    let rrule = RRuleExpression::try_from("freq=daily;count=2").unwrap();
    let dtstart = RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap();
    let schedule_timezone = ScheduleTimezone::try_from("UTC").unwrap();
    let rrule_timezone = RRuleTimezone::try_from("UTC".to_string()).unwrap();

    assert_eq!(every.as_duration(), Duration::from_secs(30));
    assert_eq!(
        EveryDuration::from_secs(31).unwrap().as_duration(),
        Duration::from_secs(31)
    );
    assert_eq!(
        EveryDuration::try_from(Duration::from_secs(32)).unwrap().as_duration(),
        Duration::from_secs(32)
    );
    assert_eq!(
        EveryDuration::try_from(33).unwrap().as_duration(),
        Duration::from_secs(33)
    );
    assert!(EveryDuration::new(Duration::ZERO).is_err());
    assert!(EveryDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).is_err());
    assert_eq!(CronExpression::try_from("0 0 * * * *").unwrap().as_str(), "0 0 * * * *");
    assert_eq!(cron.as_str(), "0 0 * * * *");
    assert_eq!(cron.into_string(), "0 0 * * * *");
    assert_eq!(
        RRuleExpression::try_from("FREQ=DAILY;COUNT=2".to_string())
            .unwrap()
            .as_str(),
        "FREQ=DAILY;COUNT=2"
    );
    assert_eq!(rrule.as_str(), "FREQ=DAILY;COUNT=2");
    assert_eq!(rrule.into_string(), "FREQ=DAILY;COUNT=2");
    assert_eq!(dtstart.as_str(), "2026-01-01T00:00:00Z");
    assert_eq!(
        dtstart.to_datetime(),
        chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap()
    );
    assert_eq!(dtstart.into_string(), "2026-01-01T00:00:00Z");
    assert_eq!(schedule_timezone.as_str(), "UTC");
    assert_eq!(schedule_timezone.id(), "UTC");
    assert_eq!(schedule_timezone.tzdb_version().as_str(), chrono_tz::IANA_TZDB_VERSION);
    assert_eq!(schedule_timezone.into_string(), "UTC");
    assert_eq!(rrule_timezone.as_str(), "UTC");
    assert_eq!(rrule_timezone.tzdb_version().as_str(), chrono_tz::IANA_TZDB_VERSION);
    assert_eq!(rrule_timezone.into_string(), "UTC");
    let version = TzdbVersion::new("2025b").unwrap();
    assert_eq!(version.as_str(), "2025b");
    assert_eq!(version.clone().into_string(), "2025b");
    assert_eq!(
        TimeZone::with_tzdb_version("America/New_York", version)
            .unwrap()
            .tzdb_version()
            .as_str(),
        "2025b"
    );
    assert!(ScheduleTimezone::try_from("UTC\n").is_err());
    assert!(RRuleTimezone::try_from("Nope/Zone").is_err());
    assert!(TzdbVersion::new("").is_err());
    assert!(TzdbVersion::new("2025").is_err());
    assert!(TzdbVersion::new("25b").is_err());
    assert!(TzdbVersion::new("2025B").is_err());
}

#[test]
fn rrule_validation_covers_invalid_shapes() {
    for raw in [
        "",
        "RRULE:",
        "FREQ=DAILY\nCOUNT=2",
        "FREQDAILY",
        "FREQ=DAILY;COUNT=2;UNTIL=20260101T000000Z",
        "FREQ=DAILY;EXRULE=FREQ=WEEKLY",
        "FREQ=DAILY;RSCALE=GREGORIAN",
        "FREQ=DAILY;SKIP=OMIT",
        "FREQ=NOPE;COUNT=2",
    ] {
        assert!(RRuleExpression::new(raw).is_err(), "{raw}");
    }
}

#[test]
fn schedule_headers_cover_helpers_and_reserved_names() {
    let headers = ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap();
    let message_headers = MessageHeaders::new([("x-kind", "heartbeat")]).unwrap();
    let from_message_headers = ScheduleHeaders::try_from(message_headers).unwrap();

    assert!(!headers.is_empty());
    assert_eq!(headers.as_slice()[0].name().as_str(), "x-kind");
    assert_eq!(headers.clone().into_message_headers().as_slice().len(), 1);
    assert_eq!(MessageHeaders::from(from_message_headers).as_slice().len(), 1);
    assert!(ScheduleHeaders::new([("Nats-Schedule", "value")]).is_err());
    assert!(ScheduleHeaders::new([("bad name", "value")]).is_err());
    assert!(ScheduleHeaders::new([("x-kind", "bad\nvalue")]).is_err());
}

#[test]
fn route_sampling_and_delivery_cover_conversions() {
    let route = DeliveryRoute::try_from("agent.run".to_string()).unwrap();
    let route_ref = DeliveryRoute::try_from("agent.reply").unwrap();
    let subject = SamplingSubject::try_from("agent.events".to_string()).unwrap();
    let subject_ref = SamplingSubject::try_from("agent.replay").unwrap();
    let ttl = TtlDuration::new(Duration::from_secs(60)).unwrap();
    let source = SamplingSource::latest_from_subject("agent.events").unwrap();
    let delivery = Delivery::NatsEvent {
        route: route.clone(),
        ttl: Some(ttl),
        source: Some(source.clone()),
    };

    assert_eq!(route.as_str(), "agent.run");
    assert_eq!(route.as_token().as_str(), "agent.run");
    assert_eq!(route_ref.as_str(), "agent.reply");
    assert_eq!(subject.as_str(), "agent.events");
    assert_eq!(subject_ref.as_str(), "agent.replay");
    assert_eq!(ttl.as_duration(), Duration::from_secs(60));
    assert_eq!(
        TtlDuration::from_secs(61).unwrap().as_duration(),
        Duration::from_secs(61)
    );
    assert_eq!(
        TtlDuration::try_from(Duration::from_secs(62)).unwrap().as_duration(),
        Duration::from_secs(62)
    );
    assert_eq!(
        TtlDuration::try_from(63).unwrap().as_duration(),
        Duration::from_secs(63)
    );
    assert!(TtlDuration::new(Duration::ZERO).is_err());
    assert!(TtlDuration::new(PROTOBUF_DURATION_MAX + Duration::from_nanos(1)).is_err());
    assert_eq!(source.subject().as_str(), "agent.events");
    assert!(DeliveryRoute::try_from("bad*route").is_err());
    assert!(SamplingSubject::try_from("bad>subject").is_err());
    assert!(matches!(
        Delivery::nats_event("agent.run").unwrap(),
        Delivery::NatsEvent { .. }
    ));
    assert!(matches!(
        ScheduleEventSamplingSource::from(source),
        ScheduleEventSamplingSource::LatestFromSubject { .. }
    ));
    assert!(matches!(
        ScheduleEventDelivery::from(delivery),
        ScheduleEventDelivery::NatsMessage {
            ttl: Some(_),
            source: Some(_),
            ..
        }
    ));
}

#[test]
fn schedule_and_message_conversions_cover_all_variants() {
    let at = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let rrule_dt = RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap();
    let schedules = [
        Schedule::At { at },
        Schedule::every(Duration::from_secs(30)).unwrap(),
        Schedule::cron("0 0 * * * *", Some("UTC".to_string())).unwrap(),
        Schedule::RRule {
            dtstart: rrule_dt.clone(),
            rrule: RRuleExpression::new("FREQ=DAILY;COUNT=2").unwrap(),
            timezone: Some(RRuleTimezone::new("UTC").unwrap()),
            rdate: vec![rrule_dt.clone()],
            exdate: vec![rrule_dt],
        },
    ];

    for schedule in schedules {
        let event_schedule = ScheduleEventSchedule::from(&schedule);
        let owned_event_schedule = ScheduleEventSchedule::from(schedule);
        assert_eq!(format!("{event_schedule:?}"), format!("{owned_event_schedule:?}"));
    }

    let message = ScheduleMessage {
        content: MessageContent::json("{}"),
        headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
    };
    assert_eq!(MessageEnvelope::from(&message).headers.as_slice().len(), 1);
    assert_eq!(
        MessageEnvelope::from(message).content.content_type().as_str(),
        "application/json"
    );
}

mod proptests;
