use trogon_cron_jobs_proto::v1;

#[derive(Debug, Clone, PartialEq)]
pub enum JobEventSchedule {
    At { at: chrono::DateTime<chrono::Utc> },
    Every { every_sec: u64 },
    Cron { expr: String, timezone: Option<String> },
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
