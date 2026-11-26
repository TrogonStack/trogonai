use crate::{GuestActorIdError, GuestAgentPortError};

/// The kernel command line carries an argument the guest cannot boot with.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum GuestBootConfigError {
    #[error(transparent)]
    Actor(#[from] GuestActorIdError),
    #[error(transparent)]
    AgentPort(#[from] GuestAgentPortError),
}
