//! Resolve a verified caller identity from inbound NATS messages.
//!
//! Production path: parse the `X-A2a-Caller-Jwt` header carrying a minted
//! NATS User JWT, verify the signature against the gateway's signing-key
//! source, and surface the resulting `SpiceDbSubject` plus audience.
//!
//! Labs-only fallback: when `A2A_GATEWAY_TRUST_CALLER_HEADERS=true`, also
//! accept a deserialized `X-A2a-Spicedb-Principal` header or a raw
//! `X-A2a-Caller-Id` header. The fallback is intentionally gated behind a
//! one-time warning at boot so it cannot silently ship to production.

use std::sync::Arc;
use std::sync::Once;

use a2a_auth_callout::{
    AccountName, AudienceAccount, CALLER_JWT_HEADER_NAME, SigningKeySource, SpiceDbPrincipal, UserJwtClaims,
    caller_jwt_header::CallerJwtHeaderValue,
};
use async_nats::HeaderMap;
use tracing::warn;
use trogon_std::env::ReadEnv;

use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};

pub const ENV_GATEWAY_TRUST_CALLER_HEADERS: &str = "A2A_GATEWAY_TRUST_CALLER_HEADERS";
pub const ENV_GATEWAY_JWT_AUDIENCE: &str = "A2A_GATEWAY_JWT_AUDIENCE";

static TRUST_CALLER_HEADERS_WARN: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustCallerHeaders(bool);

impl TrustCallerHeaders {
    pub fn is_enabled(self) -> bool {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayCallerIdentityPolicy {
    pub trust_caller_headers: TrustCallerHeaders,
}

impl GatewayCallerIdentityPolicy {
    pub fn production_default() -> Self {
        Self {
            trust_caller_headers: TrustCallerHeaders(false),
        }
    }
}

pub fn gateway_caller_identity_policy<E: ReadEnv>(env: &E) -> GatewayCallerIdentityPolicy {
    let trust_caller_headers = TrustCallerHeaders(gateway_trust_caller_headers_enabled(env));
    if trust_caller_headers.is_enabled() {
        TRUST_CALLER_HEADERS_WARN.call_once(|| {
            warn!(
                env = ENV_GATEWAY_TRUST_CALLER_HEADERS,
                "gateway caller identity: labs-only header-trust fallback active; disable for production"
            );
        });
    }
    GatewayCallerIdentityPolicy { trust_caller_headers }
}

fn gateway_trust_caller_headers_enabled<E: ReadEnv>(env: &E) -> bool {
    env.var(ENV_GATEWAY_TRUST_CALLER_HEADERS)
        .ok()
        .is_some_and(|flag| matches!(flag.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

pub fn gateway_jwt_audience<E: ReadEnv>(env: &E, prefix_fallback: &str) -> AudienceAccount {
    env.var(ENV_GATEWAY_JWT_AUDIENCE)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| AccountName::new(s.trim()))
        .unwrap_or_else(|| AccountName::new(prefix_fallback))
}

/// Outcome of verifying a caller's identity.
///
/// Construction enforces an invariant: the wrapped `SpiceDbPrincipal` MUST
/// resolve to a `spicedb_subject` (i.e. callers cannot fabricate a "verified"
/// identity with a missing subject). Downstream `identity_from_verified` can
/// rely on that without re-checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCallerIdentity {
    principal: SpiceDbPrincipal,
    audience: Option<AudienceFromPrincipal>,
}

impl VerifiedCallerIdentity {
    /// Build a `VerifiedCallerIdentity`. Returns `None` if the principal
    /// does not resolve to a `spicedb_subject` — that's the invariant
    /// downstream code depends on.
    pub fn new(principal: SpiceDbPrincipal, audience: Option<AudienceFromPrincipal>) -> Option<Self> {
        principal.spicedb_subject()?;
        Some(Self { principal, audience })
    }

    pub fn principal(&self) -> &SpiceDbPrincipal {
        &self.principal
    }

    pub fn audience(&self) -> Option<&AudienceFromPrincipal> {
        self.audience.as_ref()
    }

    /// Build from a principal alone, deriving the audience from its
    /// `aud`/`audience` field. Returns `None` if `spicedb_subject` is missing.
    pub fn from_principal(principal: SpiceDbPrincipal) -> Option<Self> {
        let audience = audience_from_principal(&principal);
        Self::new(principal, audience)
    }

    /// Build from a principal plus an explicitly-verified audience (typically
    /// the JWT `aud` claim that signature verification already validated).
    /// Falls back to `audience_from_principal` if the explicit audience is
    /// `None` so callers don't lose the principal-derived value.
    pub fn from_principal_and_verified_audience(
        principal: SpiceDbPrincipal,
        verified_audience: Option<AudienceFromPrincipal>,
    ) -> Option<Self> {
        let audience = verified_audience.or_else(|| audience_from_principal(&principal));
        Self::new(principal, audience)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceFromPrincipal(String);

impl AudienceFromPrincipal {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait MessageCallerIdentitySource {
    fn verified_caller_identity(&self, message: &async_nats::Message) -> Option<VerifiedCallerIdentity>;
}

pub struct JwtHeaderCallerIdentitySource {
    signing_key_source: Arc<dyn SigningKeySource>,
    audience: AudienceAccount,
}

impl JwtHeaderCallerIdentitySource {
    pub fn new(signing_key_source: Arc<dyn SigningKeySource>, audience: AudienceAccount) -> Self {
        Self {
            signing_key_source,
            audience,
        }
    }
}

impl MessageCallerIdentitySource for JwtHeaderCallerIdentitySource {
    fn verified_caller_identity(&self, message: &async_nats::Message) -> Option<VerifiedCallerIdentity> {
        let headers = message.headers.as_ref()?;
        let raw = header_value(headers, CALLER_JWT_HEADER_NAME)?;
        let token = match CallerJwtHeaderValue::parse(raw) {
            Ok(token) => token,
            Err(_) => {
                // The header was present but didn't match the wire format; an
                // attacker passing garbage shouldn't look the same as the
                // header simply being absent.
                warn!(
                    header = CALLER_JWT_HEADER_NAME,
                    "gateway caller identity: failed to parse caller JWT header"
                );
                return None;
            }
        };
        let claims = match UserJwtClaims::verify_minted_user_jwt(
            token.as_str(),
            self.signing_key_source.as_ref(),
            &self.audience,
        ) {
            Ok(claims) => claims,
            Err(_) => {
                // Verification failed (signature / aud / expiry). Surface
                // it to the operator so an invalid token isn't silently
                // indistinguishable from a missing one.
                warn!(
                    header = CALLER_JWT_HEADER_NAME,
                    "gateway caller identity: caller JWT failed verification"
                );
                return None;
            }
        };
        // The JWT's `aud` claim was authenticated by `verify_minted_user_jwt`;
        // surface it as the verified audience rather than relying on
        // optional fields inside the `data` principal JSON.
        let verified_audience = Some(AudienceFromPrincipal(claims.aud.as_str().to_owned()));
        VerifiedCallerIdentity::from_principal_and_verified_audience(claims.data, verified_audience)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerIdentitySource {
    MessageCallerJwtHeader,
    HeaderPrincipal,
    HeaderCallerId,
}

impl CallerIdentitySource {
    pub fn audit_label(self) -> &'static str {
        match self {
            Self::MessageCallerJwtHeader => "jwt_header",
            Self::HeaderPrincipal => "header_principal",
            Self::HeaderCallerId => "header_trusted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtCallerIdentity {
    pub spicedb_subject: a2a_auth_callout::SpiceDbSubject,
    pub audience: Option<String>,
    pub source: CallerIdentitySource,
}

pub fn resolve_gateway_caller_identity(
    message_identity: &impl MessageCallerIdentitySource,
    message: &async_nats::Message,
    headers: &HeaderMap,
    policy: GatewayCallerIdentityPolicy,
) -> Option<JwtCallerIdentity> {
    if let Some(verified) = message_identity.verified_caller_identity(message) {
        return identity_from_verified(verified, CallerIdentitySource::MessageCallerJwtHeader);
    }

    if !policy.trust_caller_headers.is_enabled() {
        return None;
    }

    if let Some(principal) = principal_from_headers(headers)
        && let Some(subject) = principal.spicedb_subject()
    {
        return Some(JwtCallerIdentity {
            spicedb_subject: subject,
            audience: audience_from_principal(&principal).map(|a| a.0),
            source: CallerIdentitySource::HeaderPrincipal,
        });
    }

    let raw = header_value(headers, GATEWAY_CALLER_ID_HEADER)?;

    Some(JwtCallerIdentity {
        spicedb_subject: a2a_auth_callout::SpiceDbSubject::new(raw),
        audience: None,
        source: CallerIdentitySource::HeaderCallerId,
    })
}

fn identity_from_verified(verified: VerifiedCallerIdentity, source: CallerIdentitySource) -> Option<JwtCallerIdentity> {
    let subject = verified.principal.spicedb_subject()?;
    Some(JwtCallerIdentity {
        spicedb_subject: subject,
        audience: verified.audience.map(|a| a.0),
        source,
    })
}

pub fn gateway_audit_caller_attribution(identity: Option<JwtCallerIdentity>) -> (String, Option<String>) {
    match identity {
        Some(id) => (
            id.spicedb_subject.as_str().to_owned(),
            Some(id.source.audit_label().to_owned()),
        ),
        None => ("_".to_owned(), None),
    }
}

/// Wire shape for the labs-only `X-A2a-Spicedb-Principal` header. Validates
/// the minimum structure (`spicedb_subject` present and non-empty) at the
/// boundary before any of it becomes a `SpiceDbPrincipal` value-object —
/// otherwise arbitrary header JSON would be promoted into a domain type
/// without per-type validation.
#[derive(Debug, serde::Deserialize)]
struct PrincipalHeaderWire {
    spicedb_subject: String,
    #[serde(default)]
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

fn principal_from_headers(headers: &HeaderMap) -> Option<SpiceDbPrincipal> {
    let raw = header_value(headers, GATEWAY_PRINCIPAL_HEADER)?;
    let wire: PrincipalHeaderWire = serde_json::from_str(&raw).ok()?;
    if wire.spicedb_subject.trim().is_empty() {
        return None;
    }
    // Re-assemble the validated JSON. Domain code reads
    // `principal.spicedb_subject()` and the audience helpers; keep `extra`
    // around so the labs path still surfaces fields like `aud`/`audience`.
    let mut object = wire.extra;
    object.insert(
        "spicedb_subject".to_owned(),
        serde_json::Value::String(wire.spicedb_subject),
    );
    Some(SpiceDbPrincipal(serde_json::Value::Object(object)))
}

fn audience_from_principal(principal: &SpiceDbPrincipal) -> Option<AudienceFromPrincipal> {
    principal
        .0
        .get("aud")
        .or_else(|| principal.0.get("audience"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| AudienceFromPrincipal(s.to_owned()))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| std::str::from_utf8(value.as_ref()).ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests;
