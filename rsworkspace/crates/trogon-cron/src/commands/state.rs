use buffa::EnumValue;
use trogon_cron_jobs_proto::{state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStateProtoError {
    MissingStateValue,
    UnsupportedEvent,
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for JobStateProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingStateValue => f.write_str("protobuf state is missing its state value"),
            Self::UnsupportedEvent => f.write_str("protobuf job event is not supported by command state"),
            Self::UnknownStateValue { value } => write!(f, "protobuf state '{value}' is unknown"),
        }
    }
}

impl std::error::Error for JobStateProtoError {}

pub(super) fn initial_state() -> state_v1::State {
    state_v1::State {
        state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
    }
}

pub(super) fn evolve(state: state_v1::State, event: &v1::JobEvent) -> Result<state_v1::State, JobStateProtoError> {
    let current_state = state_value(&state)?;
    let next_state = match &event.event {
        Some(v1::__buffa::oneof::job_event::Event::JobAdded(inner)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else if job_added_status(inner)? == v1::JobStatus::JOB_STATUS_DISABLED {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(v1::__buffa::oneof::job_event::Event::JobPaused(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
            }
        }
        Some(v1::__buffa::oneof::job_event::Event::JobResumed(_)) => {
            if current_state == state_v1::StateValue::STATE_VALUE_DELETED {
                state_v1::StateValue::STATE_VALUE_DELETED
            } else {
                state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
            }
        }
        Some(v1::__buffa::oneof::job_event::Event::JobRemoved(_)) => state_v1::StateValue::STATE_VALUE_DELETED,
        None => return Err(JobStateProtoError::UnsupportedEvent),
    };

    Ok(state_v1::State {
        state: Some(EnumValue::from(next_state)),
    })
}

pub(super) fn state_value(state: &state_v1::State) -> Result<state_v1::StateValue, JobStateProtoError> {
    let Some(value) = state.state.as_ref() else {
        return Err(JobStateProtoError::MissingStateValue);
    };
    let Some(value) = value.as_known() else {
        return Err(JobStateProtoError::UnknownStateValue { value: value.to_i32() });
    };
    match value {
        state_v1::StateValue::STATE_VALUE_MISSING
        | state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED
        | state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED
        | state_v1::StateValue::STATE_VALUE_DELETED => Ok(value),
        state_v1::StateValue::STATE_VALUE_UNSPECIFIED => {
            Err(JobStateProtoError::UnknownStateValue { value: value as i32 })
        }
    }
}

fn job_added_status(inner: &v1::JobAdded) -> Result<v1::JobStatus, JobStateProtoError> {
    let Some(job) = inner.job.as_option() else {
        return Err(JobStateProtoError::UnsupportedEvent);
    };
    Ok(job.status)
}

#[cfg(test)]
mod tests {
    use buffa::{EnumValue, MessageField};
    use trogon_eventsourcing::testing::{TestCase, decider};

    use super::*;
    use crate::commands::domain::JobId;
    use crate::commands::{PauseJobCommand, PauseJobDecisionError, RemoveJobCommand, ResumeJobCommand};

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
            .then(trogon_eventsourcing::events![paused.clone()]);

        TestCase::new(decider::<ResumeJobCommand>())
            .given([added.clone()])
            .given([paused.clone()])
            .when(ResumeJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_DISABLED)),
            })
            .then(trogon_eventsourcing::events![resumed.clone()]);

        TestCase::new(decider::<RemoveJobCommand>())
            .given([added])
            .given([paused])
            .given([resumed])
            .when(RemoveJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_PRESENT_ENABLED)),
            })
            .then(trogon_eventsourcing::events![job_removed()]);
    }

    #[test]
    fn pause_from_missing_history_returns_domain_error() {
        TestCase::new(decider::<PauseJobCommand>())
            .given_no_history()
            .when(PauseJobCommand::new(job_id("backup")))
            .state(state_v1::State {
                state: Some(EnumValue::from(state_v1::StateValue::STATE_VALUE_MISSING)),
            })
            .then_error(PauseJobDecisionError::JobNotFound { id: job_id("backup") });
    }
}
