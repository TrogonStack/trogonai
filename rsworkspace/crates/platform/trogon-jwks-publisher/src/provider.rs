//! Agent Provider issuance of `aa-agent+jwt` tokens.
//!
//! Per "Obtaining an Agent Token": an Agent Provider mints an identity token
//! binding an [`AgentIdentifier`] to the agent's public key (`cnf.jwk`, RFC
//! 7800) so resources can later verify proof-of-possession. This module
//! replaces hand-rolled `jsonwebtoken::encode` call sites with a typed API
//! that can't construct a claim set missing a required field.

use std::time::{SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use trogon_identity_types::aauth::{AgentClaims, Cnf, DWK_AGENT, TYP_AGENT};

/// Validated `aauth:local@domain` agent identifier.
///
/// Per "Agent Identity":
/// - local part: lowercase ASCII letters, digits, `-`, `_`, `+`, `.`; MUST
///   NOT be empty; MUST NOT exceed 255 characters. `+` is reserved as the
///   sub-agent delimiter (`parent-local+discriminator`) -- a top-level
///   identifier's local part must not itself contain `+`.
/// - domain part: a syntactically plausible domain name (non-empty labels
///   separated by `.`, no leading/trailing dot, no empty labels, ASCII).
/// - Comparison is exact string, case-sensitive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentIdentifier(String);

impl AgentIdentifier {
    const SCHEME_PREFIX: &'static str = "aauth:";
    const MAX_LOCAL_LEN: usize = 255;

    pub fn new(raw: impl Into<String>) -> Result<Self, AgentIdentifierError> {
        let value = raw.into();
        let rest = value
            .strip_prefix(Self::SCHEME_PREFIX)
            .ok_or_else(|| AgentIdentifierError::MissingScheme(value.clone()))?;

        let (local, domain) = rest
            .split_once('@')
            .ok_or_else(|| AgentIdentifierError::MissingAt(value.clone()))?;

        Self::validate_local(local)?;
        Self::validate_domain(domain)?;

        Ok(Self(value))
    }

    fn validate_local(local: &str) -> Result<(), AgentIdentifierError> {
        if local.is_empty() {
            return Err(AgentIdentifierError::EmptyLocal);
        }
        if local.len() > Self::MAX_LOCAL_LEN {
            return Err(AgentIdentifierError::LocalTooLong(local.len()));
        }
        if !local.chars().all(Self::is_valid_local_char) {
            return Err(AgentIdentifierError::InvalidLocalChar(local.to_string()));
        }
        Ok(())
    }

    fn is_valid_local_char(c: char) -> bool {
        c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '+' | '.')
    }

    fn validate_domain(domain: &str) -> Result<(), AgentIdentifierError> {
        if domain.is_empty() {
            return Err(AgentIdentifierError::EmptyDomain);
        }
        if !domain.is_ascii() || domain.chars().any(char::is_whitespace) {
            return Err(AgentIdentifierError::InvalidDomain(domain.to_string()));
        }
        if domain.starts_with('.') || domain.ends_with('.') {
            return Err(AgentIdentifierError::InvalidDomain(domain.to_string()));
        }
        for label in domain.split('.') {
            if label.is_empty() {
                return Err(AgentIdentifierError::InvalidDomain(domain.to_string()));
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentIdentifierError {
    #[error("agent identifier {0:?} must start with \"aauth:\"")]
    MissingScheme(String),
    #[error("agent identifier {0:?} must contain \"@\" separating local part and domain")]
    MissingAt(String),
    #[error("agent identifier local part must not be empty")]
    EmptyLocal,
    #[error("agent identifier local part must not exceed 255 characters, got {0}")]
    LocalTooLong(usize),
    #[error("agent identifier local part {0:?} contains characters outside [a-z0-9-_+.]")]
    InvalidLocalChar(String),
    #[error("agent identifier domain part must not be empty")]
    EmptyDomain,
    #[error("agent identifier domain part {0:?} is not a syntactically valid domain")]
    InvalidDomain(String),
}

/// Non-empty Agent Provider issuer URL (`iss` claim).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIssuer(String);

impl ProviderIssuer {
    pub fn new(raw: impl Into<String>) -> Result<Self, ProviderIssuerError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ProviderIssuerError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderIssuerError {
    #[error("provider issuer must not be empty")]
    Empty,
}

/// Non-empty key id (`kid` header).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyId(String);

impl KeyId {
    pub fn new(raw: impl Into<String>) -> Result<Self, KeyIdError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(KeyIdError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyIdError {
    #[error("key id must not be empty")]
    Empty,
}

/// Positive token lifetime in seconds. Per "Obtaining an Agent Token", an
/// agent token's lifetime SHOULD NOT exceed 24h; this type only enforces the
/// mechanical "must be positive" rule and leaves the 24h ceiling as policy
/// for callers to apply if they need to enforce it strictly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenTtl(u64);

impl TokenTtl {
    pub fn new(seconds: i64) -> Result<Self, TokenTtlError> {
        if seconds <= 0 {
            return Err(TokenTtlError::NotPositive(seconds));
        }
        Ok(Self(seconds.unsigned_abs()))
    }

    #[must_use]
    pub fn as_secs(self) -> u64 {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TokenTtlError {
    #[error("token ttl must be positive, got {0}")]
    NotPositive(i64),
}

/// Agent Provider signing identity: an ES256 encoding key plus the `kid` /
/// `iss` values every token it mints must carry.
pub struct AgentProviderKey {
    encoding_key: EncodingKey,
    kid: KeyId,
    iss: ProviderIssuer,
}

impl AgentProviderKey {
    #[must_use]
    pub fn new(encoding_key: EncodingKey, kid: KeyId, iss: ProviderIssuer) -> Self {
        Self { encoding_key, kid, iss }
    }
}

/// Optional Person Server HTTPS URL (`ps` claim), validated non-empty when
/// present. Per "Obtaining an Agent Token", `ps` names the Person Server the
/// agent bootstraps consent through, when the Agent Provider knows one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonServerUrl(String);

impl PersonServerUrl {
    pub fn new(raw: impl Into<String>) -> Result<Self, PersonServerUrlError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(PersonServerUrlError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn into_inner(self) -> String {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PersonServerUrlError {
    #[error("person server url must not be empty when provided")]
    Empty,
}

/// Request to mint an `aa-agent+jwt` for a specific agent identity and its
/// public confirmation key.
pub struct AgentTokenRequest {
    pub sub: AgentIdentifier,
    /// Agent's public JWK, embedded verbatim into `cnf.jwk` per RFC 7800.
    /// Kept as `serde_json::Value` to match `Cnf::jwk`'s type -- this crate
    /// does not validate JWK shape beyond what the caller already produced
    /// (e.g. via `trogon-aauth-sdk`'s public key derivation).
    pub agent_jwk: serde_json::Value,
    pub ttl: TokenTtl,
    pub ps: Option<PersonServerUrl>,
}

/// Errors while minting an `aa-agent+jwt`.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("failed to encode aa-agent+jwt: {0}")]
    Encode(#[source] jsonwebtoken::errors::Error),
    #[error("system clock is before unix epoch")]
    ClockBeforeEpoch,
}

/// Agent Provider: mints `aa-agent+jwt` tokens under a fixed signing key,
/// `kid`, and `iss`.
pub struct AgentProvider {
    key: AgentProviderKey,
}

impl AgentProvider {
    #[must_use]
    pub fn new(key: AgentProviderKey) -> Self {
        Self { key }
    }

    /// Mint an `aa-agent+jwt` per "Obtaining an Agent Token".
    ///
    /// Builds `typ: aa-agent+jwt`, sets `dwk` to `aauth-agent.json`
    /// (`DWK_AGENT`), and embeds `req.agent_jwk` into `cnf.jwk` (RFC 7800) so
    /// resources can bind proof-of-possession to the agent's key.
    pub fn mint(&self, req: AgentTokenRequest) -> Result<String, ProviderError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ProviderError::ClockBeforeEpoch)?;
        let iat = i64::try_from(now.as_secs()).unwrap_or(i64::MAX);
        let exp = iat.saturating_add(i64::try_from(req.ttl.as_secs()).unwrap_or(i64::MAX));

        let mut header = Header::new(Algorithm::ES256);
        header.typ = Some(TYP_AGENT.to_string());
        header.kid = Some(self.key.kid.as_str().to_string());

        let claims = AgentClaims {
            iss: self.key.iss.as_str().to_string(),
            sub: req.sub.as_str().to_string(),
            jti: uuid::Uuid::new_v4().to_string(),
            iat,
            exp,
            dwk: DWK_AGENT.to_string(),
            cnf: Cnf { jwk: req.agent_jwk },
            ps: req.ps.map(PersonServerUrl::into_inner),
        };

        encode(&header, &claims, &self.key.encoding_key).map_err(ProviderError::Encode)
    }
}

#[cfg(test)]
mod tests;
