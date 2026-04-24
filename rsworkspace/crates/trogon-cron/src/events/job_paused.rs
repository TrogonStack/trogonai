use serde::{Deserialize, Serialize};
use trogon_eventsourcing::EventType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobPaused {
    pub id: String,
}

impl EventType for JobPaused {
    fn event_type(&self) -> &'static str {
        "job_paused"
    }
}
