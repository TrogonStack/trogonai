use thiserror::Error;

/// A `SANDBOX_ENTITY_MODE` value that is not `off`, `shadow` or `on`.
#[derive(Debug, Clone, Error, Eq, PartialEq)]
#[error("invalid SANDBOX_ENTITY_MODE `{value}`: expected off, shadow or on")]
pub struct SandboxEntityModeError {
    pub value: String,
}
