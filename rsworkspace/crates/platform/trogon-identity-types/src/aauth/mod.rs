//! AAuth (draft-hardt-aauth-protocol) wire types: token typ values, claim sets,
//! HTTP/NATS PoP envelopes, requirement headers.
//!
//! This module is transport-agnostic. Verification and signing live in
//! `trogon-aauth-verify`; key management in `trogon-aauth-person` /
//! `trogon-jwks-publisher`.

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
/// `dwk` value for an Access Server, per "Auth Token Structure" / "Access Server Metadata".
pub const DWK_ACCESS: &str = "aauth-access.json";

/// Returns the well-known path (per RFC 8615) for a `dwk` value, e.g. `aauth-agent.json`
/// becomes `/.well-known/aauth-agent.json`. See "Metadata Documents".
#[must_use]
pub fn well_known_path(dwk: &str) -> String {
    format!("/.well-known/{dwk}")
}

pub mod delegation;
pub mod error;
pub mod federation;
pub mod headers;
pub mod login;
pub mod mission;
pub mod person_server;

pub use delegation::Act;

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
    /// Delegation chain per "Auth Token Structure" / "Delegation Chain". Optional so
    /// existing minted tokens (no chaining) keep parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<Act>,
    /// Confirmation claim per "Auth Token Structure" (verification rule 7 requires
    /// `cnf.jwk`). Optional so existing minted tokens keep parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cnf: Option<Cnf>,
}

/// Parsed `AAuth-Requirement` header value. See draft "Requirement Responses" /
/// "Requirement Values" for the full registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Requirement {
    AuthToken {
        resource_token: String,
    },
    Interaction {
        url: String,
        code: Option<String>,
    },
    Clarification,
    /// Deprecated wire form kept for backward compatibility with already-minted
    /// Trogon responses; the draft's wire value is `requirement=approval`, modeled
    /// as [`Requirement::Approval`].
    ApprovalPending,
    /// `requirement=agent-token` per "Agent Token Required": AAuth agent token
    /// required for identity-only access. Carries no parameters.
    AgentToken,
    /// `requirement=approval` per "Requirement Values" / "Approval Pending": approval
    /// pending from another party, no user direction required.
    Approval,
    /// `requirement=claims` per "Claims Required": identity claims required before
    /// the request can proceed.
    Claims,
    Other {
        raw: String,
    },
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
            Requirement::AgentToken => "requirement=agent-token".into(),
            Requirement::Approval => "requirement=approval".into(),
            Requirement::Claims => "requirement=claims".into(),
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
            Some("agent-token") => Requirement::AgentToken,
            Some("approval") => Requirement::Approval,
            Some("claims") => Requirement::Claims,
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
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AAuthParseError {
    #[error("aauth: missing field {0}")]
    MissingField(&'static str),
    #[error("aauth: invalid number for {0}")]
    InvalidNumber(&'static str),
}

#[cfg(test)]
mod tests;
