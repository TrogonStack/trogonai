/// A document path that [`DocumentPath`](crate::DocumentPath) refuses.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum DocumentPathError {
    #[error("document path is empty")]
    Empty,
    #[error("document path must be relative to the project dir, got \"{path}\"")]
    Absolute { path: String },
}
