use serde::{Deserialize, Serialize};

use super::ScheduleEventSamplingSource;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleEventDelivery {
    NatsEvent {
        route: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<ScheduleEventSamplingSource>,
    },
}
