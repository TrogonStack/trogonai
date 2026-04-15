use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use trogon_eventsourcing::ExpectedState;

use crate::error::CronError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct JobSpec {
    pub id: String,
    #[serde(default)]
    pub state: JobEnabledState,
    pub schedule: ScheduleSpec,
    pub delivery: DeliverySpec,
    pub payload: serde_json::Value,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum JobEnabledState {
    #[default]
    Enabled,
    Disabled,
}

impl JobEnabledState {
    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", content = "version", rename_all = "snake_case")]
pub enum JobWriteCondition {
    MustNotExist,
    MustBeAtVersion(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobWriteState {
    current_version: Option<u64>,
    exists: bool,
}

impl JobWriteState {
    pub const fn new(current_version: Option<u64>, exists: bool) -> Self {
        Self {
            current_version,
            exists,
        }
    }

    pub const fn current_version(self) -> Option<u64> {
        self.current_version
    }

    pub const fn exists(self) -> bool {
        self.exists
    }
}

impl JobWriteCondition {
    pub fn ensure(self, id: &str, state: JobWriteState) -> Result<(), CronError> {
        match self {
            Self::MustNotExist if !state.exists() => Ok(()),
            Self::MustBeAtVersion(expected) if state.current_version() == Some(expected) => Ok(()),
            expected => Err(CronError::OptimisticConcurrencyConflict {
                id: id.to_string(),
                expected: expected.into(),
                current_version: state.current_version(),
            }),
        }
    }
}

impl From<JobWriteCondition> for ExpectedState {
    fn from(value: JobWriteCondition) -> Self {
        match value {
            JobWriteCondition::MustNotExist => Self::NoStream,
            JobWriteCondition::MustBeAtVersion(version) => Self::StreamRevision(version),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleSpec {
    At {
        at: DateTime<Utc>,
    },
    Every {
        every_sec: u64,
    },
    Cron {
        expr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timezone: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DeliverySpec {
    NatsEvent {
        route: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        headers: BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ttl_sec: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<SamplingSource>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SamplingSource {
    LatestFromSubject { subject: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use trogon_eventsourcing::Snapshot;

    #[test]
    fn job_spec_state_defaults_to_enabled() {
        let raw = r#"{
            "id": "heartbeat",
            "schedule": { "type": "every", "every_sec": 30 },
            "delivery": { "type": "nats_event", "route": "agent.run" },
            "payload": { "kind": "heartbeat" }
        }"#;

        let job: JobSpec = serde_json::from_str(raw).unwrap();

        assert_eq!(job.state, JobEnabledState::Enabled);
    }

    #[test]
    fn job_spec_round_trips() {
        let job = JobSpec {
            id: "compact".to_string(),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Cron {
                expr: "0 */5 * * * *".to_string(),
                timezone: Some("UTC".to_string()),
            },
            delivery: DeliverySpec::NatsEvent {
                route: "workflow.compact".to_string(),
                headers: BTreeMap::from([("x-kind".to_string(), "compact".to_string())]),
                ttl_sec: Some(30),
                source: Some(SamplingSource::LatestFromSubject {
                    subject: "sensors.latest".to_string(),
                }),
            },
            payload: serde_json::json!({"workflow": "compact"}),
            metadata: BTreeMap::from([("owner".to_string(), "ops".to_string())]),
        };

        let json = serde_json::to_string(&job).unwrap();
        let decoded: JobSpec = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, job);
    }

    #[test]
    fn empty_metadata_and_headers_are_omitted() {
        let job = JobSpec {
            id: "compact".to_string(),
            state: JobEnabledState::Enabled,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        };

        let json = serde_json::to_string(&job).unwrap();

        assert!(!json.contains("\"metadata\""));
        assert!(!json.contains("\"headers\""));
        assert!(json.contains("\"state\":\"enabled\""));
    }

    #[test]
    fn snapshot_round_trips() {
        let snapshot = Snapshot::new(
            9,
            JobSpec {
                id: "compact".to_string(),
                state: JobEnabledState::Enabled,
                schedule: ScheduleSpec::Every { every_sec: 30 },
                delivery: DeliverySpec::NatsEvent {
                    route: "agent.run".to_string(),
                    headers: BTreeMap::new(),
                    ttl_sec: None,
                    source: None,
                },
                payload: serde_json::json!({"kind": "heartbeat"}),
                metadata: BTreeMap::new(),
            },
        );

        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: Snapshot<JobSpec> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn job_enabled_state_helpers_work() {
        assert!(JobEnabledState::Enabled.is_enabled());
        assert_eq!(JobEnabledState::Enabled.as_str(), "enabled");
        assert!(!JobEnabledState::Disabled.is_enabled());
        assert_eq!(JobEnabledState::Disabled.as_str(), "disabled");
    }

    #[test]
    fn write_condition_ensures_expected_versions() {
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(None, false))
            .unwrap();
        JobWriteCondition::MustBeAtVersion(3)
            .ensure("alpha", JobWriteState::new(Some(3), true))
            .unwrap();

        let error = JobWriteCondition::MustBeAtVersion(2)
            .ensure("alpha", JobWriteState::new(Some(4), true))
            .unwrap_err();
        assert!(matches!(
            error,
            CronError::OptimisticConcurrencyConflict {
                current_version: Some(4),
                ..
            }
        ));
    }

    #[test]
    fn write_condition_allows_recreating_deleted_stream() {
        JobWriteCondition::MustNotExist
            .ensure("alpha", JobWriteState::new(Some(7), false))
            .unwrap();
    }
}
