/// Why a registry handoff could not be produced or read back.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum HandoffError {
    #[error("the secret handoff is malformed")]
    Malformed,
    #[error("the sentinel registry could not be encoded for the handoff")]
    Unencodable,
}
