/// A provider snapshot id does not satisfy the wire grammar.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum NativeSnapshotIdError {
    #[error("native snapshot id must be 1 to {max} characters")]
    InvalidLength { max: usize },
    #[error("native snapshot id may only contain ASCII letters, digits, '-', '_' and '.'")]
    InvalidCharacters,
}
