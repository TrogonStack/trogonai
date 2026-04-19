use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::events::{JobEventDelivery, JobEventSchedule, JobEventState, RegisteredJobSpec};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CronJob {
    pub id: String,
    #[serde(default)]
    pub state: JobEventState,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
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
            payload: spec.payload,
            metadata: spec.metadata,
        }
    }
}
