use serde::{Deserialize, Serialize};
use trogon_eventsourcing::EventType;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobRemoved {
    pub id: String,
}

impl EventType for JobRemoved {
    fn event_type(&self) -> &'static str {
        "job_removed"
    }
}
