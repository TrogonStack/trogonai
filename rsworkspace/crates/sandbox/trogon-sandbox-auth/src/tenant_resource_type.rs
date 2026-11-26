use std::fmt;
use std::str::FromStr;

use crate::TenantResourceTypeError;

const DEFAULT_TENANT_RESOURCE_TYPE: &str = "sandbox_tenant";

/// The permission schema object type that tenants are checked as.
///
/// It holds 1 to [`TenantResourceType::MAX_LEN`] bytes, starts with a lowercase ASCII letter, and
/// continues with lowercase letters, digits, or underscores. The default is `sandbox_tenant`.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct TenantResourceType(String);

impl TenantResourceType {
    /// Longest resource type name accepted, in bytes.
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, TenantResourceTypeError> {
        let value = value.into();
        let Some(first) = value.bytes().next() else {
            return Err(TenantResourceTypeError::Empty);
        };
        if value.len() > Self::MAX_LEN {
            return Err(TenantResourceTypeError::TooLong { max: Self::MAX_LEN });
        }
        if !first.is_ascii_lowercase() {
            return Err(TenantResourceTypeError::InvalidStart);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(TenantResourceTypeError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TenantResourceType {
    fn default() -> Self {
        Self(DEFAULT_TENANT_RESOURCE_TYPE.to_string())
    }
}

impl FromStr for TenantResourceType {
    type Err = TenantResourceTypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl fmt::Display for TenantResourceType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
