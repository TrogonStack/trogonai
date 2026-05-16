use trogon_cron_jobs_proto::v1;

#[derive(Debug, Clone, PartialEq)]
pub enum JobEventSchedule {
    At { at: chrono::DateTime<chrono::Utc> },
    Every { every_sec: u64 },
    Cron { expr: String, timezone: Option<String> },
}

impl From<&JobEventSchedule> for v1::JobSchedule {
    fn from(value: &JobEventSchedule) -> Self {
        let kind = match value {
            JobEventSchedule::At { at } => v1::AtSchedule { at: at.to_rfc3339() }.into(),
            JobEventSchedule::Every { every_sec } => v1::EverySchedule { every_sec: *every_sec }.into(),
            JobEventSchedule::Cron { expr, timezone } => v1::CronSchedule {
                expr: expr.clone(),
                timezone: timezone.clone().unwrap_or_default(),
            }
            .into(),
        };
        v1::JobSchedule { kind: Some(kind) }
    }
}
