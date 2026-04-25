use serde::{Deserialize, Serialize};

use super::{JobEventDelivery, JobEventSchedule, JobEventStatus, MessageEnvelope};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobDetails {
    #[serde(default, rename = "state")]
    pub status: JobEventStatus,
    pub schedule: JobEventSchedule,
    pub delivery: JobEventDelivery,
    pub message: MessageEnvelope,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JobEventSamplingSource, MessageContent, MessageHeaders};

    #[test]
    fn job_details_round_trip_without_id() {
        let details = JobDetails {
            status: JobEventStatus::Enabled,
            schedule: JobEventSchedule::Every { every_sec: 30 },
            delivery: JobEventDelivery::NatsEvent {
                route: "agent.run".to_string(),
                ttl_sec: None,
                source: Some(JobEventSamplingSource::LatestFromSubject {
                    subject: "jobs.latest".to_string(),
                }),
            },
            message: MessageEnvelope {
                content: MessageContent::from_static(br#"{"kind":"heartbeat"}"#),
                headers: MessageHeaders::new([("owner", "ops")]).unwrap(),
            },
        };

        let json = serde_json::to_string(&details).unwrap();
        let decoded: JobDetails = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, details);
        assert!(!json.contains("\"id\""));
    }
}
