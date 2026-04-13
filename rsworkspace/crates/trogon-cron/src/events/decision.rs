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
        DeleteJobCommand, DeliverySpec, JobSpec, JobStreamState, JobWriteCondition, PutJobCommand,
        ScheduleSpec, SetJobStateCommand, initial_state,
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
    fn put_job_decides_registration_from_initial_state() {
        let state = initial_state();
        let command = PutJobCommand::new(
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
    fn put_job_rejects_existing_stream() {
        let state = JobStreamState::try_from(job("backup", JobEnabledState::Enabled)).unwrap();
        let command = PutJobCommand::new(
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
    fn set_job_state_rejects_noop_changes() {
        let state = JobStreamState::try_from(job("backup", JobEnabledState::Enabled)).unwrap();
        let command = SetJobStateCommand {
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
    fn delete_job_decides_removal_from_present_state() {
        let state = JobStreamState::try_from(job("backup", JobEnabledState::Enabled)).unwrap();
        let command = DeleteJobCommand {
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
