use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleEventSchedule {
    At {
        at: String,
    },
    Every {
        every_sec: u64,
    },
    Cron {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
    #[serde(rename = "rrule")]
    RRule {
        dtstart: String,
        rrule: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        rdate: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        exdate: Vec<String>,
    },
}
