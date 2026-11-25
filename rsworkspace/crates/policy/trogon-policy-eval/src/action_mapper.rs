use trogon_decider_runtime::{CommandPrincipal, PrincipalKind};

use crate::agent_id::AgentId;
use crate::mapping_error::MappingError;
use crate::request::Request;

/// Describes a command to the policy layer: which action it is and over which
/// chains it runs.
///
/// The authorizer cannot know either. The own chain and the lineage come from
/// the hierarchy and Session read models, which are the host's, and the action
/// comes from the command type, which is the application's.
pub trait ActionMapper<C: ?Sized> {
    /// Builds the request for this principal running this command.
    fn request(&self, principal: &CommandPrincipal, command: &C) -> Result<Request, MappingError>;
}

impl<C, F> ActionMapper<C> for F
where
    C: ?Sized,
    F: Fn(&CommandPrincipal, &C) -> Result<Request, MappingError>,
{
    fn request(&self, principal: &CommandPrincipal, command: &C) -> Result<Request, MappingError> {
        self(principal, command)
    }
}

/// The Agent a principal stands for.
///
/// Only an Agent principal has one. A person or a service has no subject for
/// bindings to attach to, and giving them a synthetic one would invent
/// authority.
pub fn agent_of(principal: &CommandPrincipal) -> Result<AgentId, MappingError> {
    match principal.kind() {
        PrincipalKind::Agent => Ok(AgentId::new(principal.id().as_str())?),
        kind @ (PrincipalKind::Person | PrincipalKind::Service) => Err(MappingError::PrincipalNotAnAgent(kind)),
    }
}
