//! AAuth (draft-hardt-aauth-protocol) wire types: token typ values, claim sets,
//! HTTP/NATS PoP envelopes, requirement headers.
//!
//! This module is transport-agnostic. Verification and signing live in
//! `trogon-aauth-verify`; key management in `trogon-aauth-person` /
//! `trogon-jwks-publisher`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::constants::{DWK_ACCESS, DWK_AGENT, DWK_PERSON, DWK_RESOURCE, TYP_AGENT, TYP_AUTH, TYP_RESOURCE};

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
///
/// Issuer-side construction goes through [`Cnf::public`], which is the only
/// constructor; the field is private so no caller can assemble one around it.
/// Deserialization is deliberately exempt: a peer's inbound `cnf` is parsed as
/// sent, because what a peer put in its own confirmation claim is not this
/// type's call to reject, and verification reads only the public parameters.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cnf {
    /// Embedded JWK. Stored as serde_json::Value so this crate avoids depending on
    /// `jsonwebtoken`. Verifier-side parses into `jsonwebtoken::jwk::Jwk`.
    jwk: Value,
}

/// Prints only the members that say *which* key this is, never the key.
///
/// [`Cnf::public`] refuses private key material, but the deserialization path
/// above accepts whatever a peer sent, so a `Cnf` reached by that path may hold
/// a private scalar. Anything that logs a claim set at debug level would then
/// write it out, and a derived `Debug` gives no warning that this is what it
/// does. The peer's own key is theirs to mishandle; writing it into this
/// platform's logs is not.
impl std::fmt::Debug for Cnf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut out = f.debug_struct("Cnf");
        for member in crate::constants::JWK_DESCRIPTIVE_MEMBERS {
            if let Some(value) = self.jwk.get(member) {
                out.field(member, value);
            }
        }
        out.finish_non_exhaustive()
    }
}

impl Cnf {
    /// Build a confirmation claim, refusing any JWK that carries private or
    /// symmetric key material, or that is missing the public members its key
    /// type needs to be usable.
    ///
    /// Two failures are guarded here and they fail in opposite directions. A
    /// caller that passes a full keypair instead of its public half publishes
    /// the private key to every party that sees the token, with a valid
    /// signature over it; that mistake is easy to make (JWK serializers
    /// include `d` unless asked not to) and impossible to walk back once a
    /// token is issued. A caller that passes an incomplete JWK instead mints a
    /// token whose confirmation key can never satisfy a proof of possession,
    /// so every request bound to it fails at the resource with no indication
    /// that the fault is in the token rather than the request.
    pub fn public(jwk: Value) -> Result<Self, CnfError> {
        let Some(members) = jwk.as_object() else {
            return Err(CnfError::NotAnObject);
        };
        let Some(kty) = members.get("kty").and_then(Value::as_str) else {
            return Err(CnfError::MissingKeyType);
        };
        if kty.eq_ignore_ascii_case(crate::constants::KTY_OCT) {
            return Err(CnfError::SymmetricKey);
        }
        for member in crate::constants::JWK_PRIVATE_MEMBERS {
            if members.contains_key(member) {
                return Err(CnfError::PrivateKeyMaterial { member });
            }
        }
        let (kty, required): (&'static str, &[&'static str]) = match kty {
            crate::constants::KTY_EC => (crate::constants::KTY_EC, &crate::constants::JWK_REQUIRED_EC_MEMBERS),
            crate::constants::KTY_RSA => (crate::constants::KTY_RSA, &crate::constants::JWK_REQUIRED_RSA_MEMBERS),
            crate::constants::KTY_OKP => (crate::constants::KTY_OKP, &crate::constants::JWK_REQUIRED_OKP_MEMBERS),
            other => {
                return Err(CnfError::UnsupportedKeyType { kty: other.to_owned() });
            }
        };
        for member in required {
            if members.get(*member).and_then(Value::as_str).is_none_or(str::is_empty) {
                return Err(CnfError::UnusablePublicMember { kty, member });
            }
        }
        Ok(Self { jwk })
    }

    /// The embedded JWK, as it will appear in the issued token.
    #[must_use]
    pub fn jwk(&self) -> &Value {
        &self.jwk
    }
}

/// Rejections from [`Cnf::public`].
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CnfError {
    #[error("cnf.jwk must be a JSON object")]
    NotAnObject,
    #[error("cnf.jwk must name a kty")]
    MissingKeyType,
    #[error("cnf.jwk must not be a symmetric key")]
    SymmetricKey,
    #[error("cnf.jwk carries private key material in member {member:?}")]
    PrivateKeyMaterial { member: &'static str },
    #[error("cnf.jwk names key type {kty:?}, which cannot carry a confirmation key")]
    UnsupportedKeyType { kty: String },
    #[error("cnf.jwk of type {kty} needs a non-empty string member {member:?}")]
    UnusablePublicMember { kty: &'static str, member: &'static str },
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
