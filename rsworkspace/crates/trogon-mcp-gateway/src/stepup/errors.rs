//! Step-up policy evaluation errors.

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepUpError {
    MissingAuthTime,
    MalformedAuthMethod,
}

impl std::fmt::Display for StepUpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingAuthTime => f.write_str("auth_time claim is required for human step-up"),
            Self::MalformedAuthMethod => f.write_str("auth_method claim is missing or not a string"),
        }
    }
}

impl std::error::Error for StepUpError {}
