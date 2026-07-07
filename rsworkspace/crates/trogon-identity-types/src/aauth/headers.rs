//! HTTP / NATS header names used by the AAuth wire protocol.

pub const REQUIREMENT: &str = "AAuth-Requirement";
pub const ACCESS: &str = "AAuth-Access";
pub const MISSION: &str = "AAuth-Mission";
pub const CAPABILITIES: &str = "AAuth-Capabilities";

// RFC 9421 HTTP path
pub const SIGNATURE_KEY: &str = "Signature-Key";
pub const SIGNATURE_INPUT: &str = "Signature-Input";
pub const SIGNATURE: &str = "Signature";
pub const CONTENT_DIGEST: &str = "Content-Digest";

// NATS path (Trogon-defined, mirrors RFC 9421 shape).
pub const NATS_TOKEN: &str = "AAuth-Token";
pub const NATS_SIG_INPUT: &str = "AAuth-Sig-Input";
pub const NATS_SIG: &str = "AAuth-Sig";
pub const NATS_SIG_CREATED: &str = "AAuth-Sig-Created";
pub const NATS_SIG_NONCE: &str = "AAuth-Sig-Nonce";
pub const NATS_AUTH_TOKEN: &str = "AAuth-Auth-Token";

/// A single `AAuth-Capabilities` value per "AAuth-Capabilities Request Header".
/// This specification defines `interaction`, `clarification`, and `payment`;
/// `Other` preserves any capability value not recognized by this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Capability {
    Interaction,
    Clarification,
    Payment,
    Other(String),
}

impl Capability {
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Capability::Interaction => "interaction",
            Capability::Clarification => "clarification",
            Capability::Payment => "payment",
            Capability::Other(raw) => raw.as_str(),
        }
    }

    #[must_use]
    pub fn parse_token(token: &str) -> Self {
        match token.trim() {
            "interaction" => Capability::Interaction,
            "clarification" => Capability::Clarification,
            "payment" => Capability::Payment,
            other => Capability::Other(other.to_string()),
        }
    }
}

/// Parsed `AAuth-Capabilities` request header: an RFC 8941 List of Tokens, e.g.
/// `AAuth-Capabilities: interaction, clarification, payment`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities(pub Vec<Capability>);

impl Capabilities {
    /// Parse the value of an `AAuth-Capabilities` header into a typed list.
    /// Unrecognized parameters on a capability item are ignored per spec.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let items = raw
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|item| {
                let token = item.split(';').next().unwrap_or(item).trim();
                Capability::parse_token(token)
            })
            .collect();
        Capabilities(items)
    }

    /// Render the canonical wire form for the `AAuth-Capabilities` header value.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        self.0.iter().map(Capability::as_str).collect::<Vec<_>>().join(", ")
    }

    #[must_use]
    pub fn contains(&self, capability: &Capability) -> bool {
        self.0.contains(capability)
    }
}

/// The `AAuth-Access` response header value: an opaque `token68` per "AAuth-Access
/// Response Header". The token is opaque to the agent, so this wraps a `String`
/// rather than defining structure the draft does not specify.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Access(pub String);

/// Errors when parsing an `AAuth-Access` header value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccessParseError {
    #[error("aauth-access: value is empty")]
    Empty,
    #[error("aauth-access: value contains whitespace or control characters")]
    InvalidCharacters,
}

impl Access {
    /// Parse an `AAuth-Access` header value. Per "AAuth-Access Response Header",
    /// recipients MUST reject empty values and values containing embedded
    /// whitespace or control characters.
    pub fn parse(raw: &str) -> Result<Self, AccessParseError> {
        if raw.is_empty() {
            return Err(AccessParseError::Empty);
        }
        if raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(AccessParseError::InvalidCharacters);
        }
        Ok(Access(raw.to_string()))
    }

    #[must_use]
    pub fn to_header_value(&self) -> String {
        self.0.clone()
    }

    /// Render the `Authorization: AAuth <token>` header value used to present
    /// this token back to the resource on subsequent requests.
    #[must_use]
    pub fn to_authorization_header_value(&self) -> String {
        format!("AAuth {}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_parse_and_render_round_trip() {
        let raw = "interaction, clarification, payment";
        let caps = Capabilities::parse(raw);
        assert_eq!(
            caps.0,
            vec![Capability::Interaction, Capability::Clarification, Capability::Payment]
        );
        assert_eq!(caps.to_header_value(), raw);
    }

    #[test]
    fn capabilities_parse_unknown_value_as_other() {
        let caps = Capabilities::parse("interaction, quantum-teleport");
        assert_eq!(
            caps.0,
            vec![Capability::Interaction, Capability::Other("quantum-teleport".into())]
        );
    }

    #[test]
    fn capabilities_ignores_unknown_parameters() {
        let caps = Capabilities::parse("interaction;future-param=1, payment");
        assert_eq!(caps.0, vec![Capability::Interaction, Capability::Payment]);
    }

    #[test]
    fn capability_other_as_str_returns_the_raw_token() {
        let cap = Capability::Other("quantum-teleport".into());
        assert_eq!(cap.as_str(), "quantum-teleport");
    }

    #[test]
    fn capabilities_to_header_value_round_trips_unknown_capability() {
        let raw = "interaction, quantum-teleport";
        let caps = Capabilities::parse(raw);
        assert_eq!(caps.to_header_value(), raw);
    }

    #[test]
    fn capabilities_contains_checks_membership() {
        let caps = Capabilities::parse("interaction");
        assert!(caps.contains(&Capability::Interaction));
        assert!(!caps.contains(&Capability::Payment));
    }

    #[test]
    fn access_parses_opaque_token() {
        let access = Access::parse("wrapped-opaque-token-value").unwrap();
        assert_eq!(access.to_header_value(), "wrapped-opaque-token-value");
        assert_eq!(
            access.to_authorization_header_value(),
            "AAuth wrapped-opaque-token-value"
        );
    }

    #[test]
    fn access_rejects_empty_value() {
        assert_eq!(Access::parse(""), Err(AccessParseError::Empty));
    }

    #[test]
    fn access_rejects_whitespace_and_control_characters() {
        assert_eq!(Access::parse("has space"), Err(AccessParseError::InvalidCharacters));
        assert_eq!(Access::parse("has\ttab"), Err(AccessParseError::InvalidCharacters));
        assert_eq!(Access::parse("has\ncontrol"), Err(AccessParseError::InvalidCharacters));
    }
}
