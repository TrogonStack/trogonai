//! AAuth (draft-hardt-aauth-protocol) wire types: token typ values, claim sets,
//! HTTP/NATS PoP envelopes, requirement headers.
//!
//! This module is transport-agnostic. Verification and signing live in
//! `trogon-aauth-verify`; key management in `trogon-aauth-person` /
//! `trogon-jwks-publisher`.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `typ` header value identifying an agent identity token.
pub const TYP_AGENT: &str = "aa-agent+jwt";
/// `typ` header value identifying a resource challenge token.
pub const TYP_RESOURCE: &str = "aa-resource+jwt";
/// `typ` header value identifying an authorization token from a Person Server.
pub const TYP_AUTH: &str = "aa-auth+jwt";

/// `dwk` (discoverable-well-known) values used by AAuth issuers.
pub const DWK_AGENT: &str = "aauth-agent.json";
pub const DWK_RESOURCE: &str = "aauth-resource.json";
pub const DWK_PERSON: &str = "aauth-person.json";

/// HTTP / NATS header names used by the AAuth wire protocol.
pub mod headers {
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
}

/// Public-key confirmation claim (`cnf`) as carried in `aa-agent+jwt`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cnf {
    /// Embedded JWK. Stored as serde_json::Value so this crate avoids depending on
    /// `jsonwebtoken`. Verifier-side parses into `jsonwebtoken::jwk::Jwk`.
    pub jwk: Value,
}

/// Claims for an `aa-agent+jwt`. Issued by an Agent Provider at bootstrap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentClaims {
    pub iss: String,
    pub sub: String,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
    pub dwk: String,
    pub cnf: Cnf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ps: Option<String>,
}

/// Claims for an `aa-resource+jwt`. Issued by a resource as a 401/NATS-401 challenge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceClaims {
    pub iss: String,
    pub aud: String,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
    pub dwk: String,
    pub agent: String,
    pub agent_jkt: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mission: Option<MissionRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionRef {
    pub approver: String,
    pub s256: String,
}

/// Claims for an `aa-auth+jwt`. Issued by a Person Server (3-party) or AS (4-party).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthClaims {
    pub iss: String,
    pub sub: String,
    pub aud: String,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
    pub agent: String,
    pub agent_jkt: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,
}

/// Parsed `AAuth-Requirement` header value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requirement {
    AuthToken { resource_token: String },
    Interaction { url: String, code: Option<String> },
    Clarification,
    ApprovalPending,
    Other { raw: String },
}

impl Requirement {
    /// Render the canonical wire form for an HTTP response header value or NATS header.
    #[must_use]
    pub fn to_header_value(&self) -> String {
        match self {
            Requirement::AuthToken { resource_token } => {
                format!("requirement=auth-token; resource-token=\"{resource_token}\"")
            }
            Requirement::Interaction { url, code } => match code {
                Some(c) => format!("requirement=interaction; url=\"{url}\"; code=\"{c}\""),
                None => format!("requirement=interaction; url=\"{url}\""),
            },
            Requirement::Clarification => "requirement=clarification".into(),
            Requirement::ApprovalPending => "requirement=approval-pending".into(),
            Requirement::Other { raw } => raw.clone(),
        }
    }

    /// Parse the value of an `AAuth-Requirement` header into a typed enum.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let parts = split_header(raw);
        let mut requirement: Option<&str> = None;
        let mut resource_token: Option<String> = None;
        let mut url: Option<String> = None;
        let mut code: Option<String> = None;
        for (key, val) in &parts {
            match key.as_str() {
                "requirement" => requirement = Some(val.as_str()),
                "resource-token" => resource_token = Some(val.clone()),
                "url" => url = Some(val.clone()),
                "code" => code = Some(val.clone()),
                _ => {}
            }
        }
        match requirement {
            Some("auth-token") => Requirement::AuthToken {
                resource_token: resource_token.unwrap_or_default(),
            },
            Some("interaction") => Requirement::Interaction {
                url: url.unwrap_or_default(),
                code,
            },
            Some("clarification") => Requirement::Clarification,
            Some("approval-pending") => Requirement::ApprovalPending,
            _ => Requirement::Other { raw: raw.to_string() },
        }
    }
}

fn split_header(raw: &str) -> Vec<(String, String)> {
    raw.split(';')
        .filter_map(|seg| {
            let seg = seg.trim();
            if seg.is_empty() {
                return None;
            }
            let (k, v) = seg.split_once('=')?;
            let key = k.trim().to_ascii_lowercase();
            let val = strip_quotes(v.trim());
            Some((key, val))
        })
        .collect()
}

fn strip_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes.first() == Some(&b'"') && bytes.last() == Some(&b'"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// NATS PoP signature envelope, mirrored to RFC 9421 but adapted for NATS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NatsSignatureEnvelope {
    pub token: String,
    pub sig_input: String,
    pub sig: String,
    pub created: i64,
    pub nonce: String,
    pub content_digest: String,
}

impl NatsSignatureEnvelope {
    /// Compute the canonical signature base string the agent and verifier must agree on.
    #[must_use]
    pub fn canonical_base(&self, subject: &str, reply: Option<&str>, jkt: &str) -> String {
        let reply = reply.unwrap_or("");
        format!(
            concat!(
                "\"@subject\": {subject}\n",
                "\"@reply\": {reply}\n",
                "\"content-digest\": {digest}\n",
                "\"aauth-token\": {token}\n",
                "\"aauth-sig-created\": {created}\n",
                "\"aauth-sig-nonce\": {nonce}\n",
                "\"@signature-params\": {input};created={created};keyid=\"{kid}\""
            ),
            subject = subject,
            reply = reply,
            digest = self.content_digest,
            token = self.token,
            created = self.created,
            nonce = self.nonce,
            input = self.sig_input,
            kid = jkt,
        )
    }
}

/// Errors returned by AAuth parsing helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AAuthParseError {
    MissingField(&'static str),
    InvalidNumber(&'static str),
}

impl fmt::Display for AAuthParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AAuthParseError::MissingField(f0) => write!(f, "aauth: missing field {f0}"),
            AAuthParseError::InvalidNumber(f0) => write!(f, "aauth: invalid number for {f0}"),
        }
    }
}

impl std::error::Error for AAuthParseError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_token_requirement() {
        let raw = "requirement=auth-token; resource-token=\"eyJ.AAA\"";
        let req = Requirement::parse(raw);
        assert_eq!(
            req,
            Requirement::AuthToken {
                resource_token: "eyJ.AAA".into()
            }
        );
    }

    #[test]
    fn parses_interaction_requirement() {
        let raw = "requirement=interaction; url=\"https://ps.example/i/123\"; code=\"AB12\"";
        let req = Requirement::parse(raw);
        assert_eq!(
            req,
            Requirement::Interaction {
                url: "https://ps.example/i/123".into(),
                code: Some("AB12".into()),
            }
        );
    }

    #[test]
    fn renders_round_trip_auth_token() {
        let req = Requirement::AuthToken {
            resource_token: "eyJTOK".into(),
        };
        let v = req.to_header_value();
        let again = Requirement::parse(&v);
        assert_eq!(req, again);
    }

    #[test]
    fn agent_claims_serde() {
        let c = AgentClaims {
            iss: "https://ap.example".into(),
            sub: "aauth:agent-1@example".into(),
            jti: "abc".into(),
            iat: 100,
            exp: 200,
            dwk: DWK_AGENT.into(),
            cnf: Cnf {
                jwk: serde_json::json!({"kty": "EC", "crv": "P-256", "x": "X", "y": "Y"}),
            },
            ps: Some("https://ps.example".into()),
        };
        let j = serde_json::to_value(&c).unwrap();
        assert_eq!(j["dwk"], DWK_AGENT);
        assert_eq!(j["cnf"]["jwk"]["kty"], "EC");
    }

    #[test]
    fn nats_envelope_canonical_base_is_stable() {
        let env = NatsSignatureEnvelope {
            token: "TOK".into(),
            sig_input: "(\"@subject\")".into(),
            sig: "SIG".into(),
            created: 1000,
            nonce: "N".into(),
            content_digest: "sha-256=:abc:".into(),
        };
        let a = env.canonical_base("foo.bar", Some("_INBOX.1"), "JKT");
        let b = env.canonical_base("foo.bar", Some("_INBOX.1"), "JKT");
        assert_eq!(a, b);
        assert!(a.contains("\"@subject\": foo.bar"));
        assert!(a.contains("keyid=\"JKT\""));
    }
}
