use serde::{Deserialize, Serialize};

use crate::error::AuthCalloutError;

/// Token-safe NATS Account name that serves as the tenant boundary (`aud` claim).
///
/// Must not contain characters that break NATS subject tokens. Wraps a `String`
/// rather than exposing bare strings so call-sites cannot mix up account names with
/// arbitrary strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountName(String);

impl AccountName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// SpiceDB authorization principal carried in the `data` JWT claim.
///
/// Opaque to NATS and the A2A transport; consumed downstream by the gateway
/// policy engine for SpiceDB `CheckPermission` calls (Phase 1). Wraps `String`
/// for now — the full SpiceDB principal schema is defined outside this repo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpiceDbPrincipal(String);

impl SpiceDbPrincipal {
    pub fn new(principal: impl Into<String>) -> Self {
        Self(principal.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SpiceDbPrincipal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Opaque handle to the signing key used to mint User JWTs.
///
/// Phase 0 scaffold: the key is loaded from a file path supplied via env; the
/// concrete key type is kept behind this newtype so callers cannot accidentally
/// serialize or log raw key material.
#[allow(dead_code)]
pub struct SigningKey(pub(crate) jsonwebtoken::EncodingKey);

impl SigningKey {
    /// Load an HMAC-SHA256 signing key from a raw secret bytes slice.
    pub fn from_secret(secret: &[u8]) -> Self {
        Self(jsonwebtoken::EncodingKey::from_secret(secret))
    }
}

/// JWT error type for mint failures.
#[derive(Debug)]
pub struct JwtError(jsonwebtoken::errors::Error);

impl std::fmt::Display for JwtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JWT error: {}", self.0)
    }
}

impl std::error::Error for JwtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

impl From<JwtError> for AuthCalloutError {
    fn from(e: JwtError) -> Self {
        Self::JwtMint(e.to_string())
    }
}

/// Claims carried inside the Account-bound User JWT minted by the auth callout.
///
/// See `docs/A2A_AUTH_CALLOUT_SKETCH.md` §3 for the full claim layout.
///
/// - `sub` — external identity as issued by the IdP or mTLS/API-key layer.
/// - `aud` — NATS Account public key or name (tenant boundary).
/// - `data` — SpiceDB-ready principal payload; opaque to the NATS transport.
/// - `caller_id` — stable, token-safe segment reused in subject ACLs
///   (`_INBOX.{caller_id}.>`, `a2a.push.{caller_id}.>`, DLQ segments).
///
/// `exp`, `iat`, and `nbf` are standard JWT fields; include them in the
/// `jsonwebtoken::Header` / `Claims` wrapper when minting (not in this struct
/// to avoid forcing a specific time representation on callers).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserJwtClaims {
    pub sub: String,
    pub aud: AccountName,
    pub data: SpiceDbPrincipal,
    /// Stable, single-token subject segment. Must not contain `.`.
    pub caller_id: String,
}

impl UserJwtClaims {
    /// Mint a signed User JWT from these claims.
    ///
    /// The verify path and full claim enrichment (`exp`, `iat`, `nbf`,
    /// subject ACL embedding) remain `unimplemented!()` — the shape and
    /// types are the deliverable for Phase 0 scaffold.
    pub fn mint(&self, _signing_key: &SigningKey) -> Result<String, JwtError> {
        unimplemented!("JWT mint: OIDC/mTLS verify paths + ACL embedding not implemented yet")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_name_roundtrip() {
        let name = AccountName::new("acme-account");
        assert_eq!(name.as_str(), "acme-account");
        assert_eq!(name.to_string(), "acme-account");
    }

    #[test]
    fn spicedb_principal_roundtrip() {
        let p = SpiceDbPrincipal::new("user:alice");
        assert_eq!(p.as_str(), "user:alice");
        assert_eq!(p.to_string(), "user:alice");
    }

    #[test]
    fn user_jwt_claims_fields_accessible() {
        let claims = UserJwtClaims {
            sub: "oidc|acme|user-123".into(),
            aud: AccountName::new("ABCTenantKey"),
            data: SpiceDbPrincipal::new(r#"{"principal_type":"user","tenant_ref":"acme"}"#),
            caller_id: "usr_abc123".into(),
        };
        assert_eq!(claims.sub, "oidc|acme|user-123");
        assert_eq!(claims.aud.as_str(), "ABCTenantKey");
        assert!(!claims.caller_id.contains('.'));
    }

    #[test]
    fn signing_key_from_secret_does_not_panic() {
        let _key = SigningKey::from_secret(b"test-secret-not-for-production");
    }
}
