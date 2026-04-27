use trogon_cron_jobs_proto::{state_v1, v1};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStateProtoError {
    MissingEvent,
    MissingJobDetails,
    UnsupportedEvent,
    UnknownJobStatus { value: i32 },
    UnknownStateValue { value: i32 },
}

impl std::fmt::Display for JobStateProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEvent => f.write_str("protobuf job event is missing its oneof case"),
            Self::MissingJobDetails => f.write_str("protobuf job_added is missing job details"),
            Self::UnsupportedEvent => f.write_str("protobuf job event is not supported by command state"),
            Self::UnknownJobStatus { value } => write!(f, "protobuf job status '{value}' is unknown"),
            Self::UnknownStateValue { value } => write!(f, "protobuf state '{value}' is unknown"),
        }
    }
}

impl std::error::Error for JobStateProtoError {}

pub(crate) fn initial_state() -> state_v1::State {
    let mut state = state_v1::State::new();
    state.set_state(state_v1::StateValue::Missing);
    state
}

pub(crate) fn evolve(state: state_v1::State, event: &v1::JobEvent) -> Result<state_v1::State, JobStateProtoError> {
    let current_state = state.state();
    match current_state {
        state_v1::StateValue::Missing
        | state_v1::StateValue::PresentEnabled
        | state_v1::StateValue::PresentDisabled
        | state_v1::StateValue::Deleted => {}
        value => {
            return Err(JobStateProtoError::UnknownStateValue {
                value: i32::from(value),
            });
        }
    }

    let next_state = match event.event() {
        v1::job_event::EventOneof::JobAdded(inner) => {
            if !inner.has_job() {
                return Err(JobStateProtoError::MissingJobDetails);
            }
            if current_state == state_v1::StateValue::Deleted {
                state_v1::StateValue::Deleted
            } else {
                match i32::from(inner.job().status()) {
                    1 => state_v1::StateValue::PresentEnabled,
                    2 => state_v1::StateValue::PresentDisabled,
                    value => return Err(JobStateProtoError::UnknownJobStatus { value }),
                }
            }
        }
        v1::job_event::EventOneof::JobPaused(_) => {
            if current_state == state_v1::StateValue::Deleted {
                state_v1::StateValue::Deleted
            } else {
                state_v1::StateValue::PresentDisabled
            }
        }
        v1::job_event::EventOneof::JobResumed(_) => {
            if current_state == state_v1::StateValue::Deleted {
                state_v1::StateValue::Deleted
            } else {
                state_v1::StateValue::PresentEnabled
            }
        }
        v1::job_event::EventOneof::JobRemoved(_) => state_v1::StateValue::Deleted,
        v1::job_event::EventOneof::not_set(_) => return Err(JobStateProtoError::MissingEvent),
        _ => return Err(JobStateProtoError::UnsupportedEvent),
    };

    let mut state = state_v1::State::new();
    state.set_state(next_state);
    Ok(state)
}
