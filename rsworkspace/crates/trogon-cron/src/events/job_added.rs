use serde::{Deserialize, Serialize};
use trogon_eventsourcing::EventType;

use super::JobDetails;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobAdded {
    pub id: String,
    pub job: JobDetails,
}

impl EventType for JobAdded {
    fn event_type(&self) -> &'static str {
        "job_added"
    }
}
