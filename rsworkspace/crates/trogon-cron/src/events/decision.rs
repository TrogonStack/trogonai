use crate::JobId;
use crate::config::JobEnabledState;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobDecisionError {
    CannotRegisterExistingJob { id: JobId },
    MissingJobForStateChange { id: JobId },
    MissingJobForRemoval { id: JobId },
    StateAlreadySet { id: JobId, state: JobEnabledState },
}

impl std::fmt::Display for JobDecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CannotRegisterExistingJob { id } => {
                write!(f, "job '{id}' is already registered")
            }
            Self::MissingJobForStateChange { id } => {
                write!(f, "missing job for state change '{id}'")
            }
            Self::MissingJobForRemoval { id } => {
                write!(f, "missing job for removal '{id}'")
            }
            Self::StateAlreadySet { id, state } => {
                write!(f, "job '{id}' is already {}", state.as_str())
            }
        }
    }
}

impl std::error::Error for JobDecisionError {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use trogon_eventsourcing::{Decision, NonEmpty, decide};

    use super::*;
    use crate::{
        ChangeJobStateCommand, ChangeJobStateState, DeliverySpec, JobSpec, JobWriteCondition,
        RegisterJobCommand, RegisterJobState, RemoveJobCommand, RemoveJobState, ScheduleSpec,
    };

    fn job(id: &str, state: JobEnabledState) -> JobSpec {
        JobSpec {
            id: id.to_string(),
            state,
            schedule: ScheduleSpec::Every { every_sec: 30 },
            delivery: DeliverySpec::NatsEvent {
                route: "agent.run".to_string(),
                headers: BTreeMap::new(),
                ttl_sec: None,
                source: None,
            },
            payload: serde_json::json!({"kind": "heartbeat"}),
            metadata: BTreeMap::new(),
        }
    }

    #[test]
    fn register_job_decides_registration_from_initial_state() {
        let state = RegisterJobState::Missing;
        let command = RegisterJobCommand::new(
            job("backup", JobEnabledState::Enabled),
            JobWriteCondition::MustNotExist,
        )
        .unwrap();

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(crate::JobEvent::job_registered(job(
                "backup",
                JobEnabledState::Enabled
            ))))
        );
    }

    #[test]
    fn register_job_rejects_existing_stream() {
        let state = RegisterJobState::Present;
        let command = RegisterJobCommand::new(
            job("backup", JobEnabledState::Enabled),
            JobWriteCondition::MustNotExist,
        )
        .unwrap();

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            JobDecisionError::CannotRegisterExistingJob { .. }
        ));
    }

    #[test]
    fn change_job_state_rejects_noop_changes() {
        let state = ChangeJobStateState::Present {
            current: JobEnabledState::Enabled,
        };
        let command = ChangeJobStateCommand {
            id: JobId::parse("backup").unwrap(),
            state: JobEnabledState::Enabled,
            write_condition: JobWriteCondition::MustBeAtVersion(1),
        };

        assert!(matches!(
            decide(&state, &command).unwrap_err(),
            JobDecisionError::StateAlreadySet { .. }
        ));
    }

    #[test]
    fn remove_job_decides_removal_from_present_state() {
        let state = RemoveJobState::Present;
        let command = RemoveJobCommand {
            id: JobId::parse("backup").unwrap(),
            write_condition: JobWriteCondition::MustBeAtVersion(1),
        };

        let decision = decide(&state, &command).unwrap();
        assert_eq!(
            decision,
            Decision::Event(NonEmpty::one(crate::JobEvent::job_removed("backup")))
        );
    }
}
