use serde::{Deserialize, Serialize};

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
