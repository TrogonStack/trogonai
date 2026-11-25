/// A tag set exceeds the number of tags a lease may carry.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum SandboxTagsError {
    #[error("sandbox supports at most {max} tags, got {actual}")]
    TooMany { actual: usize, max: usize },
}
