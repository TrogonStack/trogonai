//! Third-Party Login per draft section "Third-Party Login"
//! (#third-party-login): an agent or resource redirects the user's browser
//! to the PS to authenticate, then the PS redirects back with a resource
//! token the agent exchanges at the PS's own `token_endpoint`.
//!
//! The draft places `login_endpoint` on the AGENT or the RESOURCE metadata
//! document, not on the Person Server's own metadata (`aauth-person.json`
//! defines no `login_endpoint`) -- see the crate-level "Deviations" note.
//! This module models the PS-side half of that flow: receiving the
//! [`LoginRequest`] redirect and completing it with a resource token minted
//! for the requesting party, which the caller then redirects back with.

use trogon_identity_types::aauth::login::LoginRequest;

/// Outcome of the PS handling one login redirect: the party that redirected
/// the user here (the agent, per `LoginRequest.ps` naming this PS) receives
/// a resource token to present at the PS's own `token_endpoint`, completing
/// the three-party exchange described in "Third-Party Login".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoginOutcome {
    pub resource_token: String,
    pub redirect_back_to: Option<String>,
}

/// Parses the incoming login redirect's query string per "Third-Party
/// Login": `ps`, `login_hint`, `domain_hint`, `tenant`, `start_path`.
#[must_use]
pub fn parse_login_request(query: &str) -> Option<LoginRequest> {
    LoginRequest::parse_query_string(query)
}

#[cfg(test)]
mod tests;
