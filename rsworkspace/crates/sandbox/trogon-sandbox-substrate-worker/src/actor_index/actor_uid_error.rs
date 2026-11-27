/// The actor resource UID is empty, too long, or has characters outside `[A-Za-z0-9-]`.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("invalid actor resource UID")]
pub struct ActorUidError;
