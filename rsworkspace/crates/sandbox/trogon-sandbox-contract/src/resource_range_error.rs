/// Resource bounds that no value can satisfy.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResourceRangeError {
    #[error("resource range minimum {min} exceeds maximum {max}")]
    Reversed { min: u32, max: u32 },
}
