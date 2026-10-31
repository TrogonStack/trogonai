//! The authorizer a host runs every execution through when none is configured.
//!
//! [ADR#0082](../../../../../docs/adr/0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
//! makes the authorization seam mandatory in the host: an execution is never
//! built without a principal and an authorizer, so the absence of a policy is
//! an explicit allow decision rather than a missing phase.

use trogon_decider_runtime::{AuthorizationDeniedError, CommandAuthorizer, CommandPrincipal};

/// Admits any principal the subject named, and refuses an execution that
/// carries none.
///
/// The refusal is the trait's own fail-closed default, so this type only
/// decides that every named principal may run every command.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AdmitAnyPrincipal;

impl<C: ?Sized> CommandAuthorizer<C> for AdmitAnyPrincipal {
    fn authorize(&self, _principal: &CommandPrincipal, _command: &C) -> Result<(), AuthorizationDeniedError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
