use buffa::MessageField;
use trogonai_proto::convert::{DurationConversionError, duration_from_std, timestamp_from_datetime};
use trogonai_proto::scheduler::schedules::v1;

use super::{CronExpression, EveryDuration, RRuleExpression, RRuleTimezone, ScheduleTimezone, TimeZone};

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEventSchedule {
    At {
        at: chrono::DateTime<chrono::Utc>,
    },
    Every {
        every: EveryDuration,
    },
    Cron {
        expr: CronExpression,
        timezone: Option<ScheduleTimezone>,
    },
    RRule {
        dtstart: chrono::DateTime<chrono::Utc>,
        rrule: RRuleExpression,
        timezone: Option<RRuleTimezone>,
        rdate: Vec<chrono::DateTime<chrono::Utc>>,
        exdate: Vec<chrono::DateTime<chrono::Utc>>,
    },
}

impl TryFrom<&ScheduleEventSchedule> for v1::Schedule {
    type Error = DurationConversionError;

    fn try_from(value: &ScheduleEventSchedule) -> Result<Self, Self::Error> {
        let kind = match value {
            ScheduleEventSchedule::At { at } => v1::schedule::At {
                at: MessageField::some(timestamp_from_datetime(at)),
            }
            .into(),
            ScheduleEventSchedule::Every { every } => v1::schedule::Every {
                every: MessageField::some(duration_from_std(every.as_duration())?),
            }
            .into(),
            ScheduleEventSchedule::Cron { expr, timezone } => v1::schedule::Cron {
                expr: expr.as_str().to_string(),
                timezone: timezone.as_ref().map(timezone_from).unwrap_or_default(),
            }
            .into(),
            ScheduleEventSchedule::RRule {
                dtstart,
                rrule,
                timezone,
                rdate,
                exdate,
            } => v1::schedule::RRule {
                dtstart: MessageField::some(timestamp_from_datetime(dtstart)),
                rrule: rrule.as_str().to_string(),
                timezone: timezone.as_ref().map(timezone_from).unwrap_or_default(),
                rdate: rdate.iter().map(timestamp_from_datetime).collect(),
                exdate: exdate.iter().map(timestamp_from_datetime).collect(),
            }
            .into(),
        };
        Ok(v1::Schedule { kind: Some(kind) })
    }
}

fn timezone_from(value: &TimeZone) -> MessageField<trogonai_proto::google::r#type::TimeZone> {
    MessageField::some(trogonai_proto::google::r#type::TimeZone {
        id: value.id().to_string(),
        version: value.tzdb_version().as_str().to_string(),
    })
}

#[cfg(test)]
mod tests {
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
}

fn timestamp_from(dt: &chrono::DateTime<chrono::Utc>) -> Timestamp {
    Timestamp::from_unix(dt.timestamp(), dt.timestamp_subsec_nanos() as i32)
}

fn timezone_from(value: &str) -> MessageField<trogonai_proto::google::r#type::TimeZone> {
    MessageField::some(trogonai_proto::google::r#type::TimeZone {
        id: value.to_owned(),
        version: String::new(),
    })
}
