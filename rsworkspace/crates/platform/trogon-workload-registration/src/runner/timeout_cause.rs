use super::runner_error::RunnerError;

#[derive(Clone, Copy)]
pub(super) enum TimeoutCause {
    Frame,
    Effect,
    Authority,
}

impl TimeoutCause {
    pub fn error(self) -> RunnerError {
        match self {
            Self::Frame => RunnerError::FrameTimeout,
            Self::Effect => RunnerError::OperationTimeout,
            Self::Authority => RunnerError::Expired,
        }
    }
}
