use crate::FsPolicyList;

/// A set of lists is not an [`FsPolicy`](crate::FsPolicy) the wire accepts.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum FsPolicyError {
    #[error("fs_policy.{list} supports at most {max} entries, got {got}")]
    TooManyEntries { list: FsPolicyList, max: usize, got: usize },
    #[error(
        "fs_policy.{list} glob {pattern:?} ends in a separator, so as a pattern it can match no \
         path and would deny nothing. Write {suggestion:?}, or add a \"**\" segment to match at \
         any depth"
    )]
    InertDenyGlob {
        list: FsPolicyList,
        pattern: String,
        suggestion: String,
    },
}
