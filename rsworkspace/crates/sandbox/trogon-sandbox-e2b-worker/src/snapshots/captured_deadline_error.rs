#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CapturedDeadlineError {
    #[error("invalid captured finite deadline provenance")]
    InvalidProvenance,
}
