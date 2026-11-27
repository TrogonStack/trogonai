use crate::ActorIdError;

/// Why the actor identity could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ActorIdentityError {
    #[error("actor identity is not readable at {path}: {source}")]
    Unreadable { path: String, source: std::io::Error },
    #[error("actor identity at {path} is not valid: {source}")]
    Invalid { path: String, source: ActorIdError },
}
