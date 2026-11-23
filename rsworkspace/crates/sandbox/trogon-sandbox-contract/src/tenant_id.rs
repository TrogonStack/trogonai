use std::fmt;

use serde::{Deserialize, Serialize};

use crate::TenantIdError;

/// The tenant that owns a lease.
///
/// It serializes as a JSON string. Deserialization stays unchecked so tenants that predate the
/// [`TenantId::new`] rules, including ones containing `:` or `/`, keep working.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct TenantId(String);

impl TenantId {
    /// Longest tenant identifier [`TenantId::new`] accepts, in bytes.
    pub const MAX_LEN: usize = 128;

    /// Validates a tenant identifier that is being introduced, such as when an API key is minted.
    ///
    /// It rejects an empty value, one longer than [`TenantId::MAX_LEN`] bytes, or one containing a
    /// control character.
    pub fn new(value: impl Into<String>) -> Result<Self, TenantIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(TenantIdError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(TenantIdError::TooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(TenantIdError::ControlCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<TenantId> for String {
    fn from(value: TenantId) -> Self {
        value.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
