use buffa::MessageField;
use buffa_types::google::protobuf::{Duration, Timestamp};
use trogonai_proto::scheduler::schedules::v1;

#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEventSchedule {
    At {
        at: chrono::DateTime<chrono::Utc>,
    },
    Every {
        every_sec: u64,
    },
    Cron {
        expr: String,
        timezone: Option<String>,
    },
    RRule {
        dtstart: chrono::DateTime<chrono::Utc>,
        rrule: String,
        timezone: Option<String>,
        rdate: Vec<chrono::DateTime<chrono::Utc>>,
        exdate: Vec<chrono::DateTime<chrono::Utc>>,
    },
}

impl From<&ScheduleEventSchedule> for v1::Schedule {
    fn from(value: &ScheduleEventSchedule) -> Self {
        let kind = match value {
            ScheduleEventSchedule::At { at } => v1::schedule::At {
                at: MessageField::some(timestamp_from(at)),
            }
            .into(),
            ScheduleEventSchedule::Every { every_sec } => v1::schedule::Every {
                every: MessageField::some(Duration {
                    seconds: *every_sec as i64,
                    nanos: 0,
                    ..Duration::default()
                }),
            }
            .into(),
            ScheduleEventSchedule::Cron { expr, timezone } => v1::schedule::Cron {
                expr: expr.clone(),
                timezone: timezone.as_deref().map(timezone_from).unwrap_or_default(),
            }
            .into(),
            ScheduleEventSchedule::RRule {
                dtstart,
                rrule,
                timezone,
                rdate,
                exdate,
            } => v1::schedule::RRule {
                dtstart: MessageField::some(timestamp_from(dtstart)),
                rrule: rrule.clone(),
                timezone: timezone.as_deref().map(timezone_from).unwrap_or_default(),
                rdate: rdate.iter().map(timestamp_from).collect(),
                exdate: exdate.iter().map(timestamp_from).collect(),
            }
            .into(),
        };
        v1::Schedule { kind: Some(kind) }
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
