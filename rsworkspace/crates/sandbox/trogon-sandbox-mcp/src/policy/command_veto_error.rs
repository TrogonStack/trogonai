/// Why a command was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandVetoError {
    /// Matched a deny pattern.
    #[error("the command matches the deny pattern {0:?}")]
    Denied(String),
    /// An allow list is set and nothing in it matched.
    #[error("the command matches none of the allowed patterns")]
    NotAllowed,
}
