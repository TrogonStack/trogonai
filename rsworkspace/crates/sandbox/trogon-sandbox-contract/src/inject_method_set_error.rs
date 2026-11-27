use crate::InjectMethod;

/// A list of methods is not a set [`InjectMethodSet`](crate::InjectMethodSet) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum InjectMethodSetError {
    #[error("methods must not be empty")]
    Empty,
    #[error("methods must not repeat '{0}'")]
    Duplicate(InjectMethod),
}
