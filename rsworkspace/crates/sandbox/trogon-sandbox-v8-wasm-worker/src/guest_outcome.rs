use crate::CapturedOutput;

/// How one guest execution ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestOutcome {
    Completed { output: CapturedOutput },
    Failed { output: CapturedOutput, error: String },
    TimedOut { output: CapturedOutput },
    Cancelled { output: CapturedOutput },
}

impl GuestOutcome {
    pub(crate) fn timed_out(output: CapturedOutput) -> Self {
        Self::TimedOut { output }
    }

    pub(crate) fn cancelled(output: CapturedOutput) -> Self {
        Self::Cancelled { output }
    }

    pub(crate) fn failed(output: CapturedOutput, error: impl Into<String>) -> Self {
        Self::Failed {
            output,
            error: error.into(),
        }
    }

    /// Maps the result of running a script, preferring a heap-limit report over whatever the
    /// terminated script produced.
    pub(crate) fn settle(result: Result<(), String>, output: CapturedOutput, out_of_memory: Option<&str>) -> Self {
        match (out_of_memory, result) {
            (Some(message), _) => Self::failed(output, message),
            (None, Ok(())) => Self::Completed { output },
            (None, Err(error)) => Self::failed(output, error),
        }
    }

    pub(crate) fn into_output(self) -> CapturedOutput {
        match self {
            Self::Completed { output }
            | Self::Failed { output, .. }
            | Self::TimedOut { output }
            | Self::Cancelled { output } => output,
        }
    }

    /// The captured output, and why the run did not complete cleanly.
    pub(crate) fn into_result(self) -> (CapturedOutput, Result<(), String>) {
        match self {
            Self::Completed { output } => (output, Ok(())),
            Self::Failed { output, error } => (output, Err(error)),
            Self::TimedOut { output } => (output, Err("execution timed out".to_owned())),
            Self::Cancelled { output } => (output, Err("execution cancelled".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests;
