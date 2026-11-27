use crate::actor_index::ActorIndexError;

/// Why a recovery or reap pass could not read the index.
#[derive(Debug, thiserror::Error)]
pub enum RecoveryError {
    #[error("actor index unavailable: {0}")]
    Index(#[from] ActorIndexError),
}
