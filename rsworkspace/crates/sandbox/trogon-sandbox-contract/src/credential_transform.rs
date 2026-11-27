use serde::{Deserialize, Serialize};

use crate::{CredentialInjection, CredentialRequest, MatchRule};

/// Credentials-brokering rule shape for a domain rule.
///
/// For each request on the connection that matches `match_rules`, the egress proxy fetches `credential` from the
/// credential broker and injects it per `injection` before forwarding the request upstream. The entries of
/// `match_rules` combine with OR, and an empty list matches every request.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CredentialTransform {
    #[serde(default)]
    pub match_rules: Vec<MatchRule>,
    pub credential: CredentialRequest,
    pub injection: CredentialInjection,
}
