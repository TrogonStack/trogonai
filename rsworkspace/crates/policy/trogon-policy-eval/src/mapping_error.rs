use trogon_decider_runtime::PrincipalKind;

use crate::identifier::IdentifierError;
use crate::request::RequestError;

/// Why a principal and a command could not become a policy request.
///
/// The authorizer treats every one of these as a denial. A command nobody
/// described to the policy layer is not a command the policy layer admits.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MappingError {
    /// Bindings attach to Agents and nodes, so only an Agent principal maps to
    /// a subject.
    #[error("a {0} principal has no policy subject")]
    PrincipalNotAnAgent(PrincipalKind),
    /// The principal identifier is not a valid Agent identity.
    #[error("principal id is not a valid agent id: {0}")]
    InvalidAgentId(#[from] IdentifierError),
    /// The mapper described a request that is not coherent.
    #[error("mapped request is invalid: {0}")]
    InvalidRequest(#[from] RequestError),
    /// The mapper has no action for this command.
    #[error("command has no policy action")]
    UnsupportedCommand,
}
