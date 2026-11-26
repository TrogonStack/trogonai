//! The one place a certificate URI becomes an [`AgentId`].
//!
//! ADR#0085 Decision 3 leaves the certificate profile to a later ADR. Until
//! then the alias is the simplest SPIFFE-shaped form that is a bijection with
//! the agent id:
//!
//! ```text
//! spiffe://<trust-domain>/agent/<agent-id>
//! ```
//!
//! The mapping is deliberately strict: lowercase scheme, no userinfo, port,
//! query, fragment, trailing slash or percent-encoding, and a trust domain
//! equal to the configured one. Both directions also hold the whole URI to the
//! SPIFFE ID syntax, whose path segments carry only letters, digits, `.`, `-`
//! and `_`. An [`AgentId`] is wider than that (a policy document may key a
//! `did:key:` id, and a registered application id may contain `:`), so such an
//! id has no SPIFFE alias: [`agent_uri`] refuses to render it and
//! [`agent_id_from_uri`] refuses to accept it, and no non-conforming SPIFFE ID
//! is ever built, accepted or logged as one. Whatever profile replaces this
//! changes this module and nothing else: the listener only calls
//! [`agent_id_from_uri`], and the supervisor-side forwarder that issues the
//! certificates will call [`agent_uri`], so the two directions cannot drift
//! apart.

use crate::agent_id::{AgentId, AgentIdError};
use crate::constants::{AGENT_PATH_PREFIX, SPIFFE_SCHEME_PREFIX};
use crate::trust_domain::TrustDomain;
use spiffe::SpiffeId;

/// The alias a certificate carries for `agent` in `trust_domain`, or why the
/// agent has none because its id is outside the SPIFFE path alphabet.
pub fn agent_uri(agent: &AgentId, trust_domain: &TrustDomain) -> Result<String, AgentUriError> {
    let uri = format!("{SPIFFE_SCHEME_PREFIX}{trust_domain}/{AGENT_PATH_PREFIX}{agent}");
    conforming(&uri)?;
    Ok(uri)
}

/// The canonical SPIFFE ID parser must accept the URI and render it back
/// unchanged.
fn conforming(uri: &str) -> Result<(), AgentUriError> {
    match SpiffeId::new(uri) {
        Ok(id) if id.to_string() == uri => Ok(()),
        _ => Err(AgentUriError::NotSpiffeConformant),
    }
}

/// Maps a certificate URI back to the agent it names, or says why it names none.
pub fn agent_id_from_uri(uri: &str, trust_domain: &TrustDomain) -> Result<AgentId, AgentUriError> {
    let rest = uri.strip_prefix(SPIFFE_SCHEME_PREFIX).ok_or(AgentUriError::NotSpiffe)?;
    let (authority, path) = rest.split_once('/').ok_or(AgentUriError::WrongPath)?;
    if authority != trust_domain.as_str() {
        return Err(AgentUriError::WrongTrustDomain);
    }
    let agent = path.strip_prefix(AGENT_PATH_PREFIX).ok_or(AgentUriError::WrongPath)?;
    let agent = AgentId::new(agent).map_err(AgentUriError::InvalidAgentId)?;
    conforming(uri)?;
    Ok(agent)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentUriError {
    #[error("URI is not a spiffe:// identifier")]
    NotSpiffe,
    #[error("URI does not name the configured trust domain")]
    WrongTrustDomain,
    #[error("URI path is not /agent/<agent-id>")]
    WrongPath,
    #[error("URI is not a valid SPIFFE ID: a path segment may only hold letters, digits, '.', '-' and '_'")]
    NotSpiffeConformant,
    #[error("URI carries an invalid agent id")]
    InvalidAgentId(#[source] AgentIdError),
}

#[cfg(test)]
mod tests;
