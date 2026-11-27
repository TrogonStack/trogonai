/// The requested CPU or memory is outside what an actor can be given.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("actor resources require CPU in 1..=999999 millicores and nonzero memory MiB")]
pub struct ActorResourcesError;
