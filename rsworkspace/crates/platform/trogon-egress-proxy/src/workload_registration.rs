use std::time::SystemTime;

use trogon_workload_identity::{IdentityConfig, RegistrationError, RegistrationSet};

#[derive(Debug, thiserror::Error)]
pub enum RegistrationProjectionError {
    #[error("registration projection exceeds its byte bound")]
    TooLarge,
    #[error("registration projection cannot be decoded")]
    Malformed(#[source] buffa::DecodeError),
    #[error("registration projection does not match the configured scope or recovery fence")]
    Scope,
    #[error("registration projection has no current bounded validity")]
    Validity,
    #[error("registration projection is invalid")]
    Registration(#[source] RegistrationError),
}

pub fn decode_registration(
    _payload: &[u8],
    _config: &IdentityConfig,
    _now: SystemTime,
) -> Result<RegistrationSet, RegistrationProjectionError> {
    Err(RegistrationProjectionError::Validity)
}

#[cfg(test)]
mod tests;
