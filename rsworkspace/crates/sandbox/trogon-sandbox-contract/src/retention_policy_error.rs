/// A retention policy keeps a number of tags outside the allowed bounds.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum RetentionPolicyError {
    #[error("max_tags must be between {min} and {max}")]
    MaxTagsOutOfRange { min: u32, max: u32 },
}
