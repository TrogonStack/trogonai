/// A stdin chunk does not satisfy the rules [`StdinChunk`](crate::StdinChunk) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum StdinChunkError {
    #[error("stdin chunk exceeds {max} bytes")]
    TooLarge { max: usize },
}
