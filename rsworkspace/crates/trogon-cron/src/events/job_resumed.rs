use serde::{Deserialize, Serialize};
use trogon_eventsourcing::EventType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobResumed {
    pub id: String,
}

impl EventType for JobResumed {
    fn event_type(&self) -> &'static str {
        "job_resumed"
    }
}
