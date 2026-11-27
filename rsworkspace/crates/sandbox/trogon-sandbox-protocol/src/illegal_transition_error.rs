use thiserror::Error;
use trogon_sandbox_contract::EntityStateKind;

/// An entity event the current state does not allow, including a lease that does not match the bound one.
#[derive(Debug, Clone, Eq, PartialEq, Error)]
#[error("illegal entity transition from {from:?} on {event}")]
pub struct IllegalTransitionError {
    /// The state the entity was in.
    pub from: EntityStateKind,
    /// The name of the rejected event.
    pub event: &'static str,
}
