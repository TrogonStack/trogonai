//! Whether the endpoint subject leads with a tenant token, per
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md).

use std::str::FromStr;

use crate::constants::{TENANT_POSITION_ABSENT, TENANT_POSITION_WILDCARD};

/// The tenant position a host subscribes with.
///
/// A host inside a tenant's own account has no tenant token to read, so it
/// subscribes without one. A host in the platform account would answer for
/// every tenant that reaches it, so it would subscribe with a wildcard at the
/// leading token. Until decider state is scoped by tenant a host serves one
/// tenant account, and [`DeciderHost::start`](crate::DeciderHost::start)
/// refuses [`TenantPosition::Wildcard`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TenantPosition {
    /// The subject starts at the prefix.
    #[default]
    Absent,
    /// The subject starts with one tenant token ahead of the prefix.
    Wildcard,
}

impl TenantPosition {
    /// The name an operator configures this position with.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Absent => TENANT_POSITION_ABSENT,
            Self::Wildcard => TENANT_POSITION_WILDCARD,
        }
    }
}

impl std::fmt::Display for TenantPosition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}

impl FromStr for TenantPosition {
    type Err = TenantPositionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        [Self::Absent, Self::Wildcard]
            .into_iter()
            .find(|position| position.as_str() == value)
            .ok_or_else(|| TenantPositionError {
                value: value.to_owned(),
            })
    }
}

/// The configured value names no tenant position.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("tenant position {value:?} is neither {TENANT_POSITION_ABSENT:?} nor {TENANT_POSITION_WILDCARD:?}")]
pub struct TenantPositionError {
    value: String,
}

#[cfg(test)]
mod tests;
