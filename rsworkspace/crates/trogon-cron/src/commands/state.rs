use buffa::EnumValue;
use trogon_cron_jobs_proto::{JobEventCase, state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvolveError {
    MissingStateValue,
    UnsupportedEvent,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for EvolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingStateValue => f.write_str("protobuf state is missing its state value"),
            Self::UnsupportedEvent => f.write_str("protobuf job event is not supported by command state"),
            Self::UnknownStateValue { value } => write!(f, "protobuf state '{value}' is unknown"),
        }
    }
}

impl std::error::Error for EvolveError {}

pub(super) fn initial_state() -> state_v1::State {
    state_v1::State {
        state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
    }
}

pub(super) fn evolve(state: state_v1::State, event: &v1::JobEvent) -> Result<state_v1::State, EvolveError> {
    let Some(value) = state.state.as_ref() else {
        return Err(EvolveError::MissingStateValue);
    };
    let Some(current_state) = value.as_known() else {
        return Err(EvolveError::UnknownStateValue { value: value.to_i32() });
    };
    let current_state = match current_state {
        state_v1::StateValue::STATE_VALUE_MISSING
        | state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
        | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
        | state_v1::StateValue::STATE_VALUE_DELETED => current_state,
        state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
            return Err(EvolveError::UnknownStateValue {
                value: current_state as i32,
            });
        }
    };
    let next_state = match &event.event {
        Some(JobEventCase::JobAdded(inner)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else if inner.job.as_option().ok_or(EvolveError::UnsupportedEvent)?.status
                == v1::JobStatus::JOB_STATUS_DISABLED
            {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(JobEventCase::JobPaused(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            }
        }
        Some(JobEventCase::JobResumed(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(JobEventCase::JobRemoved(_)) => state_v1::StateValue::STATE_VALUE_DELETED,
        None => return Err(EvolveError::UnsupportedEvent),
    };

    Ok(state_v1::State {
        state: Some(EnumValue::from(next_state)),
    })
}

#[cfg(test)]
mod tests {
    use buffa::{EnumValue, MessageField};
    use trogon_decider::testing::{TestCase, decider};

    use super::*;
    use crate::commands::domain::JobId;
    use crate::commands::{PauseJobCommand, PauseJobDecideError, RemoveJobCommand, ResumeJobCommand};

    fn job_id(id: &str) -> JobId {
        JobId::parse(id).unwrap()
    }

    fn job_added() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(
                v1::JobAdded {
                    job: MessageField::some(v1::JobDetails {
                        status: v1::JobStatus::JOB_STATUS_ENABLED,
                        schedule: MessageField::none(),
                        delivery: MessageField::none(),
                        message: MessageField::none(),
                    }),
                }
                .into(),
            ),
        }
    }

    fn job_paused() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobPaused {}.into()),
        }
    }

    fn job_resumed() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobResumed {}.into()),
        }
    }

    fn job_removed() -> v1::JobEvent {
        v1::JobEvent {
            event: Some(v1::JobRemoved {}.into()),
        }
    }

    #[test]
    fn given_when_then_supports_state_transitions() {
        let added = job_added();
        let paused = job_paused();
        let resumed = job_resumed();

        TestCase::new(decider::<PauseJobCommand>())
            .given([added.clone()])
            .when(PauseJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            })
            .then(trogon_decider::events![paused.clone()]);

        TestCase::new(decider::<ResumeJobCommand>())
            .given([added.clone()])
            .given([paused.clone()])
            .when(ResumeJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)),
            })
            .then(trogon_decider::events![resumed.clone()]);

        TestCase::new(decider::<RemoveJobCommand>())
            .given([added])
            .given([paused])
            .given([resumed])
            .when(RemoveJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            })
            .then(trogon_decider::events![job_removed()]);
    }

    #[test]
    fn pause_from_missing_history_returns_domain_error() {
        TestCase::new(decider::<PauseJobCommand>())
            .given_no_history()
            .when(PauseJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
            })
            .then_error(PauseJobDecideError::JobNotFound { id: job_id("backup") });
    }
}
