use super::*;

macro_rules! expect_schedule_kind {
    ($kind:expr, $variant:path, $message:literal) => {
        match $kind {
            $variant(inner) => inner,
            _ => panic!($message),
        }
    };
}

fn at() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc)
}

#[test]
fn converts_at_and_every_schedules_to_proto() {
    let at_proto = v1::Schedule::try_from(&ScheduleEventSchedule::At { at: at() }).unwrap();
    let every_proto = v1::Schedule::try_from(&ScheduleEventSchedule::Every {
        every: EveryDuration::new(std::time::Duration::from_secs(30)).unwrap(),
    })
    .unwrap();

    let inner = expect_schedule_kind!(at_proto.kind.unwrap(), v1::schedule::Kind::At, "expected at schedule");
    let timestamp = inner.at.as_option().unwrap();
    assert_eq!(timestamp.seconds, 1_767_225_600);
    assert_eq!(timestamp.nanos, 0);

    let inner = expect_schedule_kind!(
        every_proto.kind.unwrap(),
        v1::schedule::Kind::Every,
        "expected every schedule"
    );
    assert_eq!(inner.every.as_option().unwrap().seconds, 30);
}

#[test]
fn converts_cron_and_rrule_schedules_to_proto() {
    let cron_proto = v1::Schedule::try_from(&ScheduleEventSchedule::Cron {
        expr: CronExpression::new("0 0 * * * *").unwrap(),
        timezone: Some(ScheduleTimezone::new("UTC").unwrap()),
    })
    .unwrap();
    let rrule_proto = v1::Schedule::try_from(&ScheduleEventSchedule::RRule {
        dtstart: at(),
        rrule: RRuleExpression::new("FREQ=DAILY;COUNT=2").unwrap(),
        timezone: Some(RRuleTimezone::new("UTC").unwrap()),
        rdate: vec![at()],
        exdate: vec![at()],
    })
    .unwrap();

    let inner = expect_schedule_kind!(
        cron_proto.kind.unwrap(),
        v1::schedule::Kind::Cron,
        "expected cron schedule"
    );
    assert_eq!(inner.expr, "0 0 * * * *");
    assert_eq!(inner.timezone.as_option().unwrap().id, "UTC");
    assert_eq!(
        inner.timezone.as_option().unwrap().version,
        chrono_tz::IANA_TZDB_VERSION
    );

    let inner = expect_schedule_kind!(
        rrule_proto.kind.unwrap(),
        v1::schedule::Kind::Rrule,
        "expected rrule schedule"
    );
    assert_eq!(inner.rrule, "FREQ=DAILY;COUNT=2");
    assert_eq!(inner.timezone.as_option().unwrap().id, "UTC");
    assert_eq!(
        inner.timezone.as_option().unwrap().version,
        chrono_tz::IANA_TZDB_VERSION
    );
    assert_eq!(inner.rdate.len(), 1);
    assert_eq!(inner.exdate.len(), 1);
}
