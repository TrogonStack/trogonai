use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::ScheduleEventSamplingSource;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleEventDelivery {
    NatsMessage {
        subject: String,
        /// Full-precision message TTL, mirroring the schedule interval's fidelity.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl: Option<Duration>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<ScheduleEventSamplingSource>,
    },
}
