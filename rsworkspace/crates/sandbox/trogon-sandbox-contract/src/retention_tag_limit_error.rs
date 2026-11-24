/// A retention policy keeps a number of tags outside the allowed bounds.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum RetentionTagLimitError {
    #[error("max_tags must be between {min} and {max}")]
    OutOfRange { min: u32, max: u32 },
}
