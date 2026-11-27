/// A string is not a region name.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum VolumeRegionError {
    #[error("value must not be empty")]
    Empty,
    #[error("value must not contain whitespace or control characters")]
    InvalidCharacter,
}
