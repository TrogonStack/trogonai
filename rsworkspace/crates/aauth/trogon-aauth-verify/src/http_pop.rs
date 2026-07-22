//! HTTP proof-of-possession verifier per draft "HTTP Message Signatures
//! Profile" (#http-message-signatures-profile), profiling RFC 9421 HTTP
//! Message Signatures for AAuth.
//!
//! This crate has no HTTP framework dependency: [`HttpRequest`] models the
//! inbound request as owned, transport-agnostic data (method, authority,
//! path, headers, body) so callers adapt from whatever HTTP stack they use
//! (`http::Request`, an axum extractor, a gateway's own request type, etc.)
//! without pulling that stack into this crate.
//!
//! ## Supported RFC 9421 component subset
//!
//! This verifier implements only the subset of RFC 9421 that the AAuth
//! profile actually uses -- it is not a general-purpose HTTP Message
//! Signatures library. Supported covered components, in the order the draft
//! lists them (#covered-components):
//!
//! - `@method` (RFC 9421 Section 2.2.1): the HTTP method, uppercased.
//! - `@authority` (RFC 9421 Section 2.2.3): the request's `Host`/authority,
//!   lowercased.
//! - `@path` (RFC 9421 Section 2.2.6): the absolute path (no query).
//! - `signature-key`: the raw `Signature-Key` header field value.
//! - `content-digest` (RFC 9530, OPTIONAL): the raw `Content-Digest` header
//!   field value, required to be covered only when the request has a body.
//! - `aauth-mission` (OPTIONAL): the raw `AAuth-Mission` header field value,
//!   required to be covered only when the request carries that header.
//!
//! `Signature-Input`, `Signature`, and `Signature-Key` are parsed only in the
//! single-member Dictionary shape the draft's examples use throughout
//! (`sig=(...)`, `sig=:...:`, `sig=jwt;jwt="..."`) -- this verifier does not
//! implement the full RFC 8941 Structured Fields grammar or multi-signature
//! requests. Header component values are taken as the raw field value with
//! leading/trailing OWS trimmed and internal whitespace normalized to a
//! single space (RFC 9421 Section 2.1's "combined field value" for a
//! non-list, non-dictionary field), which is sufficient because none of the
//! covered headers here are defined as RFC 9421 structured-field-aware
//! components.
//!
//! ## Replay
//!
//! Per "Freshness and Replay" (#freshness-and-replay), this HTTP profile
//! defines no nonce mechanism -- `created` freshness is the primary replay
//! defense, and the draft suggests an optional cache keyed by
//! `(signing-key-thumbprint, created, @method, @authority, @path)`. This
//! verifier builds that exact key and checks it against the caller-supplied
//! [`ReplayStore`], so a captured-and-replayed signature within the
//! freshness window is still rejected. This deliberately differs from the
//! NATS PoP profile ([`crate::nats_pop`]), which mints an explicit nonce
//! header -- the HTTP profile has none to reuse.

use jsonwebtoken::{Algorithm, DecodingKey, crypto::verify, jwk::Jwk};
use trogon_identity_types::aauth::headers;

use crate::jwks::JwksResolver;
use crate::replay::{ReplayError, ReplayStore};
use crate::time_source::TimeSource;
use crate::token::{TokenError, TokenVerifier, VerifiedAgent, VerifiedAuth};

/// Transport-agnostic view of an inbound HTTP request. Callers adapt from
/// their HTTP framework's request type into this struct; this crate does not
/// depend on any HTTP framework (not even the `http` crate).
#[derive(Clone, Debug)]
pub struct HttpRequest {
    /// HTTP method, e.g. `"GET"`, `"POST"`. Compared case-insensitively when
    /// building `@method` (RFC 9421 canonicalizes to uppercase).
    pub method: String,
    /// Request authority (host, optionally `:port`), without scheme --
    /// RFC 9421 `@authority` (Section 2.2.3).
    pub authority: String,
    /// Absolute request path, no query string -- RFC 9421 `@path`
    /// (Section 2.2.6).
    pub path: String,
    /// Header fields as `(name, value)` pairs, in wire order. Names are
    /// matched case-insensitively per RFC 9421 Section 2.1.
    pub headers: Vec<(String, String)>,
    /// Request body, if any. `None` and `Some(&[])` are both treated as "no
    /// body" for the purpose of requiring `content-digest` coverage.
    pub body: Option<Vec<u8>>,
}

impl HttpRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn header_count(&self, name: &str) -> usize {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .count()
    }

    fn has_body(&self) -> bool {
        self.body.as_ref().is_some_and(|b| !b.is_empty())
    }
}

/// The presenting token's `typ`, and therefore which verification path and
/// result type applies. Determined from the token's own `typ` header, not
/// asserted by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresenterKind {
    Agent,
    Auth,
}

/// Result of verifying a request presenting an `aa-auth+jwt` via
/// `Signature-Key`. Distinct from [`VerifiedAgent`] (the agent-token-
/// presenting case, reused as-is) because an auth-token presenter has
/// already completed authorization and carries `AuthClaims`, not
/// `AgentClaims`.
#[derive(Debug, Clone)]
pub struct VerifiedAuthPresenter {
    /// The verified auth token and its claims.
    pub auth: VerifiedAuth,
    /// RFC 7638 JWK Thumbprint of `cnf.jwk`, i.e. the key that signed this
    /// request.
    pub jkt: String,
}

/// Outcome of [`HttpPopVerifier::verify`]: which kind of token presented the
/// request, and its verified claims.
#[derive(Debug, Clone)]
pub enum VerifiedPresenter {
    Agent(VerifiedAgent),
    Auth(VerifiedAuthPresenter),
}

/// Errors verifying an HTTP request's proof-of-possession.
#[derive(Debug, thiserror::Error)]
pub enum HttpPopError {
    #[error("missing required header: {0}")]
    MissingHeader(&'static str),
    /// A security-sensitive header appeared more than once -- refuse rather
    /// than pick one value and let the rest go unauthenticated.
    #[error("duplicate security-sensitive header: {0}")]
    DuplicateHeader(&'static str),
    /// `Signature-Key` was present but not in the supported
    /// `sig=jwt;jwt="<token>"` shape (#keying-material: AAuth agents MUST use
    /// `scheme=jwt`).
    #[error("signature-key header is not in the supported sig=jwt;jwt=\"...\" shape")]
    UnsupportedSignatureKeyScheme,
    /// `Signature-Input` was present but not in the supported
    /// `sig=(...);created=...` shape.
    #[error("signature-input header is not in the supported sig=(...) shape")]
    MalformedSignatureInput,
    /// `Signature` was present but not in the supported `sig=:...:` shape.
    #[error("signature header is not in the supported sig=:...: shape")]
    MalformedSignature,
    /// Rule (#covered-components): the required components were not all
    /// covered by `Signature-Input`.
    #[error("signature-input does not cover required component: {0}")]
    MissingCoveredComponent(&'static str),
    /// The request has a body but `content-digest` is not covered, or vice
    /// versa the header is covered but absent.
    #[error("content-digest header required but missing")]
    MissingContentDigest,
    /// The `AAuth-Mission` header is covered by the signature but absent
    /// from the request, or present but absent from coverage while a mission
    /// claim is expected -- see [`crate::mission`] for claim-level checks.
    #[error("aauth-mission header required but missing")]
    MissingMissionHeader,
    /// `Signature-Input`'s `created` parameter is missing or not an integer.
    #[error("signature-input created parameter is missing or invalid")]
    InvalidCreated,
    /// Configured `max_skew_secs` was negative -- operator misconfiguration.
    #[error("configured max_skew_secs must be non-negative, got {0}")]
    NegativeMaxSkew(i64),
    /// The arithmetic to compare `now` and `created` overflowed `i64`.
    #[error("skew arithmetic overflowed (now={now}, created={created})")]
    SkewOverflow { now: i64, created: i64 },
    /// `created` falls outside the verifier's freshness window
    /// (#freshness-and-replay, default 60 seconds).
    #[error("signature created timestamp is outside the freshness window")]
    Skew,
    /// The presenting token was neither an `aa-agent+jwt` nor an
    /// `aa-auth+jwt`.
    #[error("presenting token typ is not supported for HTTP PoP: {0}")]
    UnsupportedPresenterTyp(String),
    #[error("agent token: {0}")]
    Agent(#[source] TokenError),
    #[error("auth token: {0}")]
    Auth(#[source] TokenError),
    /// `cnf.jwk` either failed to deserialize or named an algorithm the PoP
    /// verifier doesn't support.
    #[error("invalid cnf.jwk")]
    InvalidConfirmationKey(#[source] InvalidConfirmationKey),
    /// The signature did not verify against `cnf.jwk` over the RFC 9421
    /// canonical signature base.
    #[error("signature did not verify")]
    BadSignature,
    #[error("signature verification: {0}")]
    Verify(#[source] jsonwebtoken::errors::Error),
    /// Replay per (signing-key-thumbprint, created, @method, @authority,
    /// @path) as suggested by (#freshness-and-replay).
    #[error("signature replay detected")]
    Replay,
    #[error("replay store backend failure")]
    Backend(#[source] ReplayError),
}

/// Reason `cnf.jwk` could not be used to verify the HTTP Message Signature.
/// Mirrors [`crate::nats_pop::InvalidConfirmationKey`]; kept as a separate
/// type because the two PoP profiles are independent public surfaces.
#[derive(Debug, thiserror::Error)]
pub enum InvalidConfirmationKey {
    #[error("cnf.jwk did not deserialize")]
    Deserialize(#[source] serde_json::Error),
    #[error("cnf.jwk uses an unsupported algorithm")]
    UnsupportedAlgorithm,
    #[error("cnf.jwk could not produce a decoding key")]
    DecodingKey(#[source] jsonwebtoken::errors::Error),
    /// An auth-token presenter's token had no `cnf` claim at all -- distinct
    /// from `Deserialize`/`DecodingKey`, which apply once a `cnf.jwk` value
    /// exists but fails to parse.
    #[error("auth token has no cnf claim")]
    MissingConfirmationClaim,
    /// An auth-token presenter's `cnf.jwk` was missing `kty` or a member
    /// required for its key type, mirroring "Request-Context Binding" rule
    /// 7's structurally-incomplete rejection (#auth-token-verification).
    #[error("cnf.jwk is structurally incomplete: {0}")]
    StructurallyIncomplete(#[source] crate::jkt::JktError),
}

/// Security-sensitive headers that drive PoP verification. If any appears
/// more than once, the verifier refuses rather than silently picking one
/// value and letting the rest go unauthenticated -- mirrors the same
/// defense-in-depth rule [`crate::nats_pop`] applies.
const SECURITY_HEADERS: &[&str] = &[
    headers::SIGNATURE_KEY,
    headers::SIGNATURE_INPUT,
    headers::SIGNATURE,
    headers::CONTENT_DIGEST,
    headers::MISSION,
];

/// Verifier for HTTP requests bearing an AAuth `Signature-Key` / RFC 9421
/// HTTP Message Signature.
pub struct HttpPopVerifier<R: JwksResolver, C: TimeSource, S: ReplayStore> {
    pub token_verifier: TokenVerifier<R, C>,
    pub clock: C,
    pub replay: S,
    pub max_skew_secs: i64,
    /// The resource's own identifier (its `aud`), used only to verify an
    /// auth-token presenter's `aud` claim (mirrors "Request-Context Binding"
    /// rule 5, folded in here because [`Self::verify`] already has the
    /// verified auth claims and signing context in hand).
    pub resource_identifier: String,
}

impl<R: JwksResolver + Clone, C: TimeSource + Clone, S: ReplayStore> HttpPopVerifier<R, C, S> {
    pub fn new(jwks: R, clock: C, replay: S, resource_identifier: impl Into<String>) -> Self {
        Self {
            token_verifier: TokenVerifier::new(jwks, clock.clone()),
            clock,
            replay,
            max_skew_secs: 60,
            resource_identifier: resource_identifier.into(),
        }
    }
}

impl<R: JwksResolver, C: TimeSource, S: ReplayStore> HttpPopVerifier<R, C, S> {
    /// Verify an HTTP request's proof-of-possession per "Verification
    /// (Server)" (#verification): extracts `Signature-Key`, `Signature-Input`,
    /// `Signature`; verifies required component coverage; verifies the JWT
    /// layer of the presented token (agent or auth); verifies the HTTP
    /// Message Signature against the token's `cnf.jwk`; and enforces
    /// freshness plus the replay tuple from (#freshness-and-replay).
    pub async fn verify(&self, req: &HttpRequest) -> Result<VerifiedPresenter, HttpPopError> {
        if self.max_skew_secs < 0 {
            return Err(HttpPopError::NegativeMaxSkew(self.max_skew_secs));
        }
        ensure_no_duplicate_security_headers(req)?;

        let sig_key_raw = req
            .header(headers::SIGNATURE_KEY)
            .ok_or(HttpPopError::MissingHeader(headers::SIGNATURE_KEY))?;
        let sig_input_raw = req
            .header(headers::SIGNATURE_INPUT)
            .ok_or(HttpPopError::MissingHeader(headers::SIGNATURE_INPUT))?;
        let sig_raw = req
            .header(headers::SIGNATURE)
            .ok_or(HttpPopError::MissingHeader(headers::SIGNATURE))?;

        let jwt = parse_signature_key_jwt(sig_key_raw)?;
        let parsed_input = parse_signature_input(sig_input_raw)?;

        verify_covered_components(req, &parsed_input.components)?;

        let now = self.clock.now();
        let delta = now
            .checked_sub(parsed_input.created)
            .ok_or(HttpPopError::SkewOverflow {
                now,
                created: parsed_input.created,
            })?;
        if delta.unsigned_abs() > self.max_skew_secs.unsigned_abs() {
            return Err(HttpPopError::Skew);
        }

        let sig_b64 = parse_signature_bytes(sig_raw)?;

        let (cnf_jwk, jkt, presenter) = match presenter_kind(&jwt)? {
            PresenterKind::Agent => {
                let verified = self
                    .token_verifier
                    .verify_agent(&jwt)
                    .await
                    .map_err(HttpPopError::Agent)?;
                let cnf_jwk = verified.claims.cnf.jwk.clone();
                let jkt = verified.jkt.clone();
                (cnf_jwk, jkt, VerifiedPresenter::Agent(verified))
            }
            PresenterKind::Auth => {
                let verified = self
                    .token_verifier
                    .verify_auth(&jwt, &self.resource_identifier)
                    .await
                    .map_err(HttpPopError::Auth)?;
                let cnf = verified.claims.cnf.clone().ok_or(HttpPopError::InvalidConfirmationKey(
                    InvalidConfirmationKey::MissingConfirmationClaim,
                ))?;
                let jkt = crate::jkt::jwk_thumbprint(&cnf.jwk).map_err(|e| {
                    HttpPopError::InvalidConfirmationKey(InvalidConfirmationKey::StructurallyIncomplete(e))
                })?;
                let presenter = VerifiedPresenter::Auth(VerifiedAuthPresenter {
                    auth: verified,
                    jkt: jkt.clone(),
                });
                (cnf.jwk, jkt, presenter)
            }
        };

        let base = build_signature_base(req, &parsed_input)?;
        verify_signature_with_jwk(&cnf_jwk, base.as_bytes(), &sig_b64)?;

        let replay_key = format!(
            "http-pop:{jkt}:{}:{}:{}:{}",
            parsed_input.created, req.method, req.authority, req.path
        );
        let ttl_secs = self.max_skew_secs.saturating_mul(2).max(1);
        let ttl_u32 = u32::try_from(ttl_secs).unwrap_or(u32::MAX);
        let fresh = self
            .replay
            .check_and_insert(&replay_key, ttl_u32)
            .await
            .map_err(HttpPopError::Backend)?;
        if !fresh {
            return Err(HttpPopError::Replay);
        }

        Ok(presenter)
    }
}

fn ensure_no_duplicate_security_headers(req: &HttpRequest) -> Result<(), HttpPopError> {
    for &name in SECURITY_HEADERS {
        if req.header_count(name) > 1 {
            return Err(HttpPopError::DuplicateHeader(name));
        }
    }
    Ok(())
}

/// Determine whether the presented JWT is an `aa-agent+jwt` or
/// `aa-auth+jwt` from its header `typ`, without yet verifying its signature.
/// This only inspects the unverified header to route to the correct
/// `TokenVerifier` method; the JWT layer itself (signature, freshness, dwk)
/// is verified afterward by that method, so a forged `typ` cannot bypass
/// verification -- it can only route to a verification path that will then
/// reject the token because its actual `typ` claim/signature won't match.
fn presenter_kind(jwt: &str) -> Result<PresenterKind, HttpPopError> {
    let header = jsonwebtoken::decode_header(jwt).map_err(|_| HttpPopError::UnsupportedPresenterTyp(String::new()))?;
    match header.typ.as_deref() {
        Some(trogon_identity_types::aauth::TYP_AGENT) => Ok(PresenterKind::Agent),
        Some(trogon_identity_types::aauth::TYP_AUTH) => Ok(PresenterKind::Auth),
        other => Err(HttpPopError::UnsupportedPresenterTyp(
            other.unwrap_or("<none>").to_string(),
        )),
    }
}

/// Parsed `jwt="..."` parameter out of a `Signature-Key: sig=jwt;jwt="..."`
/// header. Only the `sig=jwt` (scheme=jwt) shape is supported
/// (#keying-material): AAuth agents MUST use `scheme=jwt`.
fn parse_signature_key_jwt(raw: &str) -> Result<String, HttpPopError> {
    let raw = raw.trim();
    let rest = raw
        .strip_prefix("sig=jwt")
        .ok_or(HttpPopError::UnsupportedSignatureKeyScheme)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(';').unwrap_or(rest).trim_start();
    let jwt_param = rest
        .strip_prefix("jwt=")
        .ok_or(HttpPopError::UnsupportedSignatureKeyScheme)?;
    let token = extract_quoted(jwt_param).ok_or(HttpPopError::UnsupportedSignatureKeyScheme)?;
    if token.is_empty() {
        return Err(HttpPopError::UnsupportedSignatureKeyScheme);
    }
    Ok(token)
}

/// Parsed `Signature: sig=:BASE64:` header, returning the base64 (padded or
/// unpadded per RFC 9421 Byte Sequence encoding) signature payload.
fn parse_signature_bytes(raw: &str) -> Result<String, HttpPopError> {
    let raw = raw.trim();
    let rest = raw.strip_prefix("sig=").ok_or(HttpPopError::MalformedSignature)?;
    let rest = rest.trim();
    let inner = rest
        .strip_prefix(':')
        .and_then(|s| s.strip_suffix(':'))
        .ok_or(HttpPopError::MalformedSignature)?;
    if inner.is_empty() {
        return Err(HttpPopError::MalformedSignature);
    }
    Ok(inner.to_string())
}

struct ParsedSignatureInput {
    components: Vec<String>,
    created: i64,
}

/// Parses `Signature-Input: sig=("@method" "@authority" ...);created=NNN`
/// into the ordered component identifier list and the `created` parameter.
/// Only the single-member-dictionary shape used throughout the draft's
/// examples is supported; other RFC 9421 parameters (`expires`, `nonce`,
/// `keyid`, `alg`, `tag`) are ignored if present since AAuth conveys the
/// equivalent information via `Signature-Key`'s embedded JWT instead.
fn parse_signature_input(raw: &str) -> Result<ParsedSignatureInput, HttpPopError> {
    let raw = raw.trim();
    let rest = raw.strip_prefix("sig=").ok_or(HttpPopError::MalformedSignatureInput)?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('(').ok_or(HttpPopError::MalformedSignatureInput)?;
    let close = rest.find(')').ok_or(HttpPopError::MalformedSignatureInput)?;
    let (list_body, after) = rest.split_at(close);
    let after = &after[1..]; // drop ')'

    let components: Vec<String> = list_body
        .split_whitespace()
        .map(|tok| extract_quoted(tok).ok_or(HttpPopError::MalformedSignatureInput))
        .collect::<Result<_, _>>()?;
    if components.is_empty() {
        return Err(HttpPopError::MalformedSignatureInput);
    }

    let mut created = None;
    for param in after.split(';') {
        let param = param.trim();
        if param.is_empty() {
            continue;
        }
        if let Some(value) = param.strip_prefix("created=") {
            created = value.trim().parse::<i64>().ok();
        }
    }
    let created = created.ok_or(HttpPopError::InvalidCreated)?;

    Ok(ParsedSignatureInput { components, created })
}

/// Extracts the interior of a `"quoted-string"` token, or returns the token
/// unchanged if it has no surrounding quotes (used for the bare `jwt` scheme
/// token in `Signature-Key`).
fn extract_quoted(tok: &str) -> Option<String> {
    let tok = tok.trim();
    if tok.len() >= 2 && tok.starts_with('"') && tok.ends_with('"') {
        Some(tok[1..tok.len() - 1].to_string())
    } else if !tok.is_empty() && !tok.contains('"') {
        Some(tok.to_string())
    } else {
        None
    }
}

/// Checks that the required covered components (#covered-components) are
/// present in `components`, plus `content-digest` when the request has a
/// body and `aauth-mission` when the request carries that header.
fn verify_covered_components(req: &HttpRequest, components: &[String]) -> Result<(), HttpPopError> {
    let has = |name: &str| components.iter().any(|c| c.eq_ignore_ascii_case(name));

    for required in ["@method", "@authority", "@path", "signature-key"] {
        if !has(required) {
            return Err(HttpPopError::MissingCoveredComponent(match required {
                "@method" => "@method",
                "@authority" => "@authority",
                "@path" => "@path",
                _ => "signature-key",
            }));
        }
    }

    if req.has_body() {
        if !has("content-digest") {
            return Err(HttpPopError::MissingContentDigest);
        }
        if req.header(headers::CONTENT_DIGEST).is_none() {
            return Err(HttpPopError::MissingContentDigest);
        }
    }

    if req.header(headers::MISSION).is_some() && !has("aauth-mission") {
        return Err(HttpPopError::MissingMissionHeader);
    }
    if has("aauth-mission") && req.header(headers::MISSION).is_none() {
        return Err(HttpPopError::MissingMissionHeader);
    }

    Ok(())
}

/// Builds the RFC 9421 canonical signature base for the supported component
/// subset (see module docs). For each covered component identifier, in the
/// order given by `Signature-Input`, appends `"<id>": <value>\n`; then a
/// final `"@signature-params": <inner-list-serialization>` line with no
/// trailing newline, per RFC 9421 Section 2.5.
fn build_signature_base(req: &HttpRequest, parsed: &ParsedSignatureInput) -> Result<String, HttpPopError> {
    let mut base = String::new();
    for component in &parsed.components {
        let value = component_value(req, component)?;
        base.push('"');
        base.push_str(component);
        base.push_str("\": ");
        base.push_str(&value);
        base.push('\n');
    }
    let quoted_components = parsed
        .components
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(" ");
    base.push_str(&format!(
        "\"@signature-params\": ({quoted_components});created={}",
        parsed.created
    ));
    Ok(base)
}

fn component_value(req: &HttpRequest, component: &str) -> Result<String, HttpPopError> {
    match component {
        "@method" => Ok(req.method.to_uppercase()),
        "@authority" => Ok(req.authority.to_lowercase()),
        "@path" => Ok(req.path.clone()),
        "signature-key" => req
            .header(headers::SIGNATURE_KEY)
            .map(normalize_field_value)
            .ok_or(HttpPopError::MissingHeader(headers::SIGNATURE_KEY)),
        "content-digest" => req
            .header(headers::CONTENT_DIGEST)
            .map(normalize_field_value)
            .ok_or(HttpPopError::MissingContentDigest),
        "aauth-mission" => req
            .header(headers::MISSION)
            .map(normalize_field_value)
            .ok_or(HttpPopError::MissingMissionHeader),
        other => req
            .header(other)
            .map(normalize_field_value)
            .ok_or(HttpPopError::MissingCoveredComponent("unsupported component")),
    }
}

/// RFC 9421 Section 2.1: a field's "combined field value" is produced by
/// stripping leading/trailing whitespace and replacing internal sequences of
/// whitespace (including any folded newlines) with a single space.
fn normalize_field_value(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn verify_signature_with_jwk(jwk_val: &serde_json::Value, base: &[u8], sig_b64: &str) -> Result<(), HttpPopError> {
    let jwk: Jwk = serde_json::from_value(jwk_val.clone())
        .map_err(|source| HttpPopError::InvalidConfirmationKey(InvalidConfirmationKey::Deserialize(source)))?;
    let alg = match &jwk.algorithm {
        jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(ec)
            if ec.curve == jsonwebtoken::jwk::EllipticCurve::P256 =>
        {
            Algorithm::ES256
        }
        jsonwebtoken::jwk::AlgorithmParameters::EllipticCurve(ec)
            if ec.curve == jsonwebtoken::jwk::EllipticCurve::P384 =>
        {
            Algorithm::ES384
        }
        jsonwebtoken::jwk::AlgorithmParameters::OctetKeyPair(okp)
            if okp.curve == jsonwebtoken::jwk::EllipticCurve::Ed25519 =>
        {
            Algorithm::EdDSA
        }
        _ => {
            return Err(HttpPopError::InvalidConfirmationKey(
                InvalidConfirmationKey::UnsupportedAlgorithm,
            ));
        }
    };
    let key = DecodingKey::from_jwk(&jwk)
        .map_err(|source| HttpPopError::InvalidConfirmationKey(InvalidConfirmationKey::DecodingKey(source)))?;
    let ok = verify(sig_b64, base, &key, alg).map_err(HttpPopError::Verify)?;
    if !ok {
        return Err(HttpPopError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
