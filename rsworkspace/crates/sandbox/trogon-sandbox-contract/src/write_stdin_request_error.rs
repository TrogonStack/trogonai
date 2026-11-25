/// A stdin write does not satisfy the rules [`WriteStdinRequest`](crate::WriteStdinRequest) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum WriteStdinRequestError {
    #[error("stdin write must carry data or eof")]
    Empty,
}
