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
mod tests;
