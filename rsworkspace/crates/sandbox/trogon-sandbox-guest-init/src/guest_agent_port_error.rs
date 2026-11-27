use std::num::ParseIntError;

/// The kernel command line carries a guest agent port that is not a nonzero 32-bit number.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum GuestAgentPortError {
    #[error("guest agent port '{value}' is not an unsigned 32-bit number")]
    NotANumber {
        value: String,
        #[source]
        source: ParseIntError,
    },
    #[error("guest agent port must be nonzero")]
    Zero,
}
