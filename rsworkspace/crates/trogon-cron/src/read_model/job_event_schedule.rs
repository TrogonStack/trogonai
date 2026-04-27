use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use trogon_cron_jobs_proto::v1;

use crate::proto::JobEventProtoError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEventSchedule {
    At {
        at: chrono::DateTime<chrono::Utc>,
    },
    Every {
        every_sec: u64,
    },
    Cron {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
}

impl From<&JobEventSchedule> for v1::JobSchedule {
    fn from(value: &JobEventSchedule) -> Self {
        let mut schedule = v1::JobSchedule::new();
        match value {
            JobEventSchedule::At { at } => {
                let mut inner = v1::AtSchedule::new();
                inner.set_at(at.to_rfc3339());
                schedule.set_at(inner);
            }
            JobEventSchedule::Every { every_sec } => {
                let mut inner = v1::EverySchedule::new();
                inner.set_every_sec(*every_sec);
                schedule.set_every(inner);
            }
            JobEventSchedule::Cron { expr, timezone } => {
                let mut inner = v1::CronSchedule::new();
                inner.set_expr(expr.as_str());
                if let Some(timezone) = timezone {
                    inner.set_timezone(timezone.as_str());
                }
                schedule.set_cron(inner);
            }
        }
        schedule
    }
}

impl TryFrom<v1::JobSchedule> for JobEventSchedule {
    type Error = JobEventProtoError;

    fn try_from(value: v1::JobSchedule) -> Result<Self, Self::Error> {
        match value.kind() {
            v1::job_schedule::KindOneof::At(inner) => {
                let at = inner.at().to_string();
                let parsed = DateTime::parse_from_rfc3339(&at)
                    .map_err(|source| JobEventProtoError::InvalidTimestamp { value: at, source })?
                    .with_timezone(&Utc);
                Ok(Self::At { at: parsed })
            }
            v1::job_schedule::KindOneof::Every(inner) => Ok(Self::Every {
                every_sec: inner.every_sec(),
            }),
            v1::job_schedule::KindOneof::Cron(inner) => Ok(Self::Cron {
                expr: inner.expr().to_string(),
                timezone: inner.has_timezone().then(|| inner.timezone().to_string()),
            }),
            v1::job_schedule::KindOneof::not_set(_) | _ => Err(JobEventProtoError::MissingScheduleKind),
        }
    }
}
