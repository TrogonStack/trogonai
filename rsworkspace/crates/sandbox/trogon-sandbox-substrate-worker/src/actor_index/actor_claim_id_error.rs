/// The actor claim identity is not a 26 character ULID.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("invalid actor claim identity")]
pub struct ActorClaimIdError;
