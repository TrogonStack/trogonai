/// Why a string is not an actor identity.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum ActorIdError {
    #[error("actor id is empty")]
    Empty,
}
