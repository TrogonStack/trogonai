//! Helpers for the 401 → exchange → retry loop.
//!
//! A resource responds with `AAuth-Requirement: requirement=auth-token; resource-token="..."`
//! (HTTP) or sets the same header on a NATS reply with code 401. The SDK parses
//! that requirement, calls the PS `/aauth/token` endpoint, and lets the caller
//! retry the original request with the resulting `aa-auth+jwt`.

use trogon_identity_types::aauth::{Requirement, headers};

use crate::error::SdkError;

/// Outcome of inspecting a denied response's headers.
#[derive(Debug, Clone)]
pub enum ChallengeOutcome {
    /// Resource expects the agent to exchange this `resource_jwt` at the PS.
    NeedsExchange { resource_jwt: String },
    /// Resource is asking for human interaction.
    Interaction { url: String, code: Option<String> },
    /// Resource sent a header we couldn't act on.
    Unsupported(Requirement),
    /// No challenge header present.
    NoChallenge,
}

/// Inspect a response header (HTTP `WWW-Authenticate`-equivalent or NATS reply
/// header) and return the next-step instruction.
#[must_use]
pub fn parse_challenge_headers(
    aauth_requirement: Option<&str>,
) -> ChallengeOutcome {
    let Some(raw) = aauth_requirement else {
        return ChallengeOutcome::NoChallenge;
    };
    match Requirement::parse(raw) {
        Requirement::AuthToken { resource_token } => ChallengeOutcome::NeedsExchange {
            resource_jwt: resource_token,
        },
        Requirement::Interaction { url, code } => ChallengeOutcome::Interaction { url, code },
        other => ChallengeOutcome::Unsupported(other),
    }
}

/// Convenience: convert [`ChallengeOutcome`] to a `Result` so callers can use `?`.
impl ChallengeOutcome {
    pub fn into_resource_jwt(self) -> Result<String, SdkError> {
        match self {
            ChallengeOutcome::NeedsExchange { resource_jwt } => Ok(resource_jwt),
            ChallengeOutcome::Interaction { url, code } => {
                Err(SdkError::Interaction { url, code })
            }
            ChallengeOutcome::Unsupported(req) => Err(SdkError::UnsupportedRequirement(req)),
            ChallengeOutcome::NoChallenge => Err(SdkError::InvalidResponse(
                "denied response missing AAuth-Requirement".into(),
            )),
        }
    }
}

/// Name of the header the SDK expects on a denied response (HTTP + NATS).
#[must_use]
pub fn requirement_header_name() -> &'static str {
    headers::REQUIREMENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_token_challenge() {
        let outcome = parse_challenge_headers(Some(
            "requirement=auth-token; resource-token=\"eyJ.AAA\"",
        ));
        match outcome {
            ChallengeOutcome::NeedsExchange { resource_jwt } => {
                assert_eq!(resource_jwt, "eyJ.AAA");
            }
            _ => panic!("expected NeedsExchange"),
        }
    }

    #[test]
    fn parses_interaction_challenge() {
        let outcome = parse_challenge_headers(Some(
            "requirement=interaction; url=\"https://ps/i/1\"",
        ));
        match outcome {
            ChallengeOutcome::Interaction { url, code } => {
                assert_eq!(url, "https://ps/i/1");
                assert!(code.is_none());
            }
            _ => panic!("expected Interaction"),
        }
    }
}
