use std::num::ParseFloatError;

/// A core count Modal cannot be asked for.
#[derive(Debug, thiserror::Error)]
pub enum CpuCoresError {
    #[error("the core count must be a positive, finite number")]
    NotPositive,
    #[error("the core count is not a number: {0}")]
    Unparsable(#[from] ParseFloatError),
}
