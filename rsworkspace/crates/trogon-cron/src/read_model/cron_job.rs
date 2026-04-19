use serde::{Deserialize, Serialize};

use crate::events::{JobEventDelivery, JobEventSchedule, JobEventState, RegisteredJobSpec};
use crate::message::{MessageContent, MessageHeaders};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJob {
    pub id: String,
    #[serde(default)]
    pub state: JobEventState,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub content: MessageContent,
    #[serde(default, skip_serializing_if = "MessageHeaders::is_empty")]
    pub headers: MessageHeaders,
}

impl CronJob {
    pub fn is_enabled(&self) -> bool {
        matches!(self.state, JobEventState::Enabled)
    }
}

impl From<(String, RegisteredJobSpec)> for CronJob {
    fn from((id, spec): (String, RegisteredJobSpec)) -> Self {
        Self {
            id,
            state: spec.state,
            schedule: spec.schedule,
            delivery: spec.delivery,
            content: spec.content,
            headers: spec.headers,
        }
    }
}
