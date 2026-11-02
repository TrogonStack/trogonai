//! HTTP proof-of-possession verifier per draft "HTTP Message Signatures
//! Profile" (#http-message-signatures-profile), profiling RFC 9421 HTTP
//! Message Signatures for AAuth, with the binding, freshness and replay
//! rules of ADR#0051.
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
//! - `content-digest` (RFC 9530): the raw `Content-Digest` header field
//!   value. Always covered, because ADR#0051 Decision 2 binds every request
//!   to SHA-256 over its exact body; a bodyless request carries the digest of
//!   the empty body.
//! - `aauth-mission` (OPTIONAL): the raw `AAuth-Mission` header field value,
//!   required to be covered only when the request carries that header.
//! - `aauth-nonce` (OPTIONAL): the raw `AAuth-Nonce` header field value,
//!   required to be covered only when the request carries that header.
//!
//! ## Body integrity
//!
//! Covering `content-digest` binds the *header value* to the signature; on
//! its own that says nothing about the bytes in `body`. This verifier
//! therefore also recomputes SHA-256 over the request body and compares,
//! exactly as [`crate::nats_pop`] does for its own profile. Without that
//! second half a captured request could have its body replaced or removed
//! while the untouched digest header kept the signature valid.
//!
//! ## Signature parameters
//!
//! `Signature-Input` is read under the RFC 8941 grammar as exactly one
//! member, `sig`, and anything outside this profile is refused rather than
//! skipped: a second member, parameters on a component, a repeated component
//! or parameter, or a parameter the profile does not define.
//!
//! - `created` and `expires` (Integers, required) bound the window in which
//!   the signature is fresh, checked by [`SignatureFreshness`].
//! - `nonce` (String, required) carries the client-generated [`Jti`].
//! - `alg` (String, optional) has to name the algorithm of `cnf.jwk`.
//! - `keyid` and `tag` (Strings, optional) are signed over but not
//!   interpreted.
//!
//! `@signature-params` is rebuilt from the parsed parameters in their RFC
//! 8941 serialization, so the signer and this verifier agree on it whatever
//! optional whitespace the field carried.
//!
//! `Signature` and `Signature-Key` are read only in the single-member shapes
//! the draft's examples use (`sig=:...:`, `sig=jwt;jwt="..."`). Header
//! component values are taken as the raw field value with leading/trailing
//! OWS trimmed and internal whitespace normalized to a single space (RFC
//! 9421 Section 2.1's "combined field value" for a non-list, non-dictionary
//! field), which is sufficient because none of the covered headers here are
//! defined as RFC 9421 structured-field-aware components.
//!
//! ## Replay
//!
//! Every admitted signature records its signing key's thumbprint and its
//! [`Jti`] in the caller-supplied [`ReplayStore`] with one atomic
//! conditional create, kept for [`SignatureFreshness::record_ttl_secs`]. A
//! `jti` is therefore admitted once per key for as long as any signature
//! carrying it could still be fresh, whatever request it is attached to,
//! and a store that cannot be consulted refuses the request.
//!
//! ## Server nonces
//!
//! With [`ServerNonces`] configured, a nonce a request binds in `AAuth-Nonce`
//! has to be one this resource issued to the request's signing key, still
//! usable and still recorded in the [`ReplayStore`]. When the policy is
//! [`NonceRequirement::Required`], a request binding no nonce, or one that
//! does not verify, is refused with [`HttpPopError::NonceRequired`], whose
//! [`NonceChallenge`] carries a fresh nonce issued to its signing key for the
//! retry, as RFC 9449 does for a missing or stale DPoP nonce. Under
//! [`NonceRequirement::Optional`] a bound nonce that does not verify is
//! refused with [`HttpPopError::InvalidNonce`]. Nonces are only ever issued
//! to a request whose signature has verified and whose `jti` was just
//! recorded, so neither an unauthenticated request nor a replayed one can
//! make the resource write a nonce record. Without [`ServerNonces`], a
//! request binding a nonce is refused, since this resource issued none.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use jsonwebtoken::jwk::{AlgorithmParameters, EllipticCurve, Jwk};
use jsonwebtoken::{Algorithm, DecodingKey, crypto::verify};
use sha2::{Digest, Sha256};
use trogon_identity_types::aauth::headers;

use crate::constants::{HTTP_SECURITY_HEADERS, SERVER_NONCE_COMPONENT, SERVER_NONCE_HEADER};
use crate::freshness::{ClockTolerance, FreshnessError, SignatureFreshness};
use crate::jwks::JwksResolver;
use crate::replay::{ReplayError, ReplayStore};
use crate::server_nonce::{NonceChallenge, NonceError, NonceIssueError, NonceRequirement, ServerNonce, ServerNonces};
use crate::time_source::TimeSource;
use crate::token::{TokenError, TokenVerifier, VerifiedAgent, VerifiedAuth};

mod signature_input;

use signature_input::SignatureInput;
pub use signature_input::{InvalidJtiError, Jti, SignatureParameter};

/// Transport-agnostic view of an inbound HTTP request. Callers adapt from
/// their HTTP framework's request type into this struct; this crate does not
/// depend on any HTTP framework (not even the `http` crate).
#[derive(Clone, Debug)]
pub struct HttpRequest {
    /// HTTP method, e.g. `"GET"`, `"POST"`. Compared case-insensitively when
    /// building `@method` (RFC 9421 canonicalizes to uppercase).
    pub method: String,
    /// Request authority (host, optionally `:port`), without scheme --
    /// RFC 9421 `@authority` (Section 2.2.3). A caller serving one resource
    /// passes its own authority here rather than the request's `Host`, so a
    /// signature made for any other resource does not verify.
    pub authority: String,
    /// Absolute request path, no query string -- RFC 9421 `@path`
    /// (Section 2.2.6).
    pub path: String,
    /// Header fields as `(name, value)` pairs, in wire order. Names are
    /// matched case-insensitively per RFC 9421 Section 2.1.
    pub headers: Vec<(String, String)>,
    /// Request body, if any. `None` digests the same as an empty body.
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
    /// `Signature-Input` was present but is not a single `sig` member this
    /// profile can read.
    #[error("signature-input header is not in the supported sig=(...) shape")]
    MalformedSignatureInput,
    /// A signature parameter this profile requires is absent.
    #[error("signature-input is missing the {0} parameter")]
    MissingSignatureParameter(SignatureParameter),
    /// A signature parameter carries no value, or a value of the wrong type.
    #[error("signature-input {0} parameter is not of the type it requires")]
    InvalidSignatureParameter(SignatureParameter),
    #[error("signature-input nonce is not a usable jti: {0}")]
    InvalidJti(#[source] InvalidJtiError),
    /// `Signature` was present but not in the supported `sig=:...:` shape.
    #[error("signature header is not in the supported sig=:...: shape")]
    MalformedSignature,
    /// Rule (#covered-components): the required components were not all
    /// covered by `Signature-Input`.
    #[error("signature-input does not cover required component: {0}")]
    MissingCoveredComponent(&'static str),
    /// `content-digest` is not covered, or is covered but absent.
    #[error("content-digest header required but missing")]
    MissingContentDigest,
    /// `Content-Digest` was present but carried no `sha-256` entry, or that
    /// entry was not a decodable Byte Sequence. Refused rather than skipped,
    /// because skipping an unparseable digest is indistinguishable from
    /// having no body integrity at all.
    #[error("content-digest header has no decodable sha-256 entry")]
    UnsupportedContentDigest,
    /// `Content-Digest` did not match SHA-256 over the request body.
    #[error("content-digest does not match the request body")]
    ContentDigestMismatch,
    /// The `AAuth-Mission` header is covered by the signature but absent
    /// from the request, or present but absent from coverage while a mission
    /// claim is expected -- see [`crate::mission`] for claim-level checks.
    #[error("aauth-mission header required but missing")]
    MissingMissionHeader,
    /// The `AAuth-Nonce` header is present but not covered by the signature,
    /// or covered but absent.
    #[error("aauth-nonce header is not covered by the signature, or is covered but absent")]
    UnboundNonce,
    /// The resource demands a server nonce and the request bound none, or
    /// one that does not verify. Carries a fresh nonce, issued to the
    /// request's signing key, for the retry.
    #[error("request binds no usable server nonce, which this resource demands")]
    NonceRequired(NonceChallenge),
    #[error("server nonce does not verify: {0}")]
    InvalidNonce(#[source] NonceError),
    #[error("server nonce could not be issued")]
    NonceIssue(#[source] NonceIssueError),
    /// The signature's `created`/`expires` window is malformed or not fresh
    /// at the verifier's clock.
    #[error("signature is not fresh: {0}")]
    Freshness(#[source] FreshnessError),
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
    InvalidConfirmationKey(#[source] InvalidConfirmationKeyError),
    /// The `alg` signature parameter names another algorithm than the one
    /// `cnf.jwk` signs with.
    #[error("signature-input alg does not name the algorithm of cnf.jwk")]
    AlgorithmMismatch,
    /// The signature did not verify against `cnf.jwk` over the RFC 9421
    /// canonical signature base.
    #[error("signature did not verify")]
    BadSignature,
    #[error("signature verification: {0}")]
    Verify(#[source] jsonwebtoken::errors::Error),
    /// The signing key already presented this `jti`.
    #[error("signature replay detected")]
    Replay,
    #[error("replay store backend failure")]
    Backend(#[source] ReplayError),
}

/// Reason `cnf.jwk` could not be used to verify the HTTP Message Signature.
/// Mirrors [`crate::nats_pop::InvalidConfirmationKeyError`]; kept as a separate
/// type because the two PoP profiles are independent public surfaces.
#[derive(Debug, thiserror::Error)]
pub enum InvalidConfirmationKeyError {
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

/// Verifier for HTTP requests bearing an AAuth `Signature-Key` / RFC 9421
/// HTTP Message Signature.
pub struct HttpPopVerifier<R: JwksResolver, C: TimeSource, S: ReplayStore> {
    pub token_verifier: TokenVerifier<R, C>,
    pub clock: C,
    pub replay: S,
    pub freshness: SignatureFreshness,
    /// The resource's own identifier (its `aud`), used to verify an
    /// auth-token presenter's `aud` claim (mirrors "Request-Context Binding"
    /// rule 5, folded in here because [`Self::verify`] already has the
    /// verified auth claims and signing context in hand), and named as the
    /// issuer of the server nonces this resource hands out.
    pub resource_identifier: String,
    pub nonces: Option<ServerNonces>,
}

impl<R: JwksResolver + Clone, C: TimeSource + Clone, S: ReplayStore> HttpPopVerifier<R, C, S> {
    /// A verifier with the default [`SignatureFreshness`], whose clock
    /// tolerance also bounds the presenting token's expiry leeway.
    pub fn new(jwks: R, clock: C, replay: S, resource_identifier: impl Into<String>) -> Self {
        let freshness = SignatureFreshness::default();
        Self {
            token_verifier: TokenVerifier::new(jwks, clock.clone()).with_leeway(u64::from(freshness.skew().secs())),
            clock,
            replay,
            freshness,
            resource_identifier: resource_identifier.into(),
            nonces: None,
        }
    }
}

impl<R: JwksResolver, C: TimeSource, S: ReplayStore> HttpPopVerifier<R, C, S> {
    #[must_use]
    pub fn with_freshness(mut self, freshness: SignatureFreshness) -> Self {
        self.freshness = freshness;
        self
    }

    /// Bounds how far past `exp`, or ahead of `iat`, the presenting token is
    /// still accepted.
    #[must_use]
    pub fn with_token_leeway(mut self, leeway: ClockTolerance) -> Self {
        self.token_verifier = self.token_verifier.with_leeway(u64::from(leeway.secs()));
        self
    }

    #[must_use]
    pub fn with_server_nonces(mut self, nonces: ServerNonces) -> Self {
        self.nonces = Some(nonces);
        self
    }

    /// Verify an HTTP request's proof-of-possession per "Verification
    /// (Server)" (#verification): extracts `Signature-Key`, `Signature-Input`,
    /// `Signature`; verifies required component coverage and the freshness of
    /// the signature window; verifies the JWT layer of the presented token
    /// (agent or auth); verifies the HTTP Message Signature against the
    /// token's `cnf.jwk`; recomputes `Content-Digest` over the body; records
    /// the signing key's `jti` so it is admitted once; and checks the server
    /// nonce the request binds, or demands one.
    pub async fn verify(&self, req: &HttpRequest) -> Result<VerifiedPresenter, HttpPopError> {
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
        let input = SignatureInput::parse(sig_input_raw)?;
        verify_covered_components(req, input.components())?;
        self.freshness
            .check(input.window(), self.clock.now())
            .map_err(HttpPopError::Freshness)?;
        let sig_b64 = parse_signature_bytes(sig_raw)?;

        let (cnf_jwk, jkt, presenter) = self.verify_presenter(&jwt).await?;

        let base = build_signature_base(req, &input)?;
        verify_signature_with_jwk(&cnf_jwk, input.alg(), base.as_bytes(), &sig_b64)?;
        verify_content_digest(req)?;

        let fresh = self
            .replay
            .check_and_insert(&replay_key(&jkt, input.jti()), self.freshness.record_ttl_secs())
            .await
            .map_err(HttpPopError::Backend)?;
        if !fresh {
            return Err(HttpPopError::Replay);
        }

        self.verify_server_nonce(req, &jkt).await?;

        Ok(presenter)
    }

    async fn verify_server_nonce(&self, req: &HttpRequest, jkt: &str) -> Result<(), HttpPopError> {
        let presented = req.header(SERVER_NONCE_HEADER);
        let Some(nonces) = &self.nonces else {
            return match presented {
                Some(_) => Err(HttpPopError::InvalidNonce(NonceError::NotIssued)),
                None => Ok(()),
            };
        };
        let refused = match presented {
            Some(raw) => match self.nonce_refusal(nonces, raw.trim(), jkt).await? {
                None => return Ok(()),
                refusal => refusal,
            },
            None => None,
        };
        match nonces.requirement() {
            NonceRequirement::Required => {
                let nonce = self.issue_nonce(nonces, jkt).await?;
                Err(HttpPopError::NonceRequired(NonceChallenge::new(nonce, refused)))
            }
            NonceRequirement::Optional => refused.map_or(Ok(()), |err| Err(HttpPopError::InvalidNonce(err))),
        }
    }

    /// Why `raw` is not a usable nonce this resource issued to the key with
    /// thumbprint `jkt`, or `None` when it is one.
    async fn nonce_refusal(
        &self,
        nonces: &ServerNonces,
        raw: &str,
        jkt: &str,
    ) -> Result<Option<NonceError>, HttpPopError> {
        let record = match nonces.check(
            raw,
            &self.resource_identifier,
            jkt,
            self.clock.now(),
            self.freshness.skew(),
        ) {
            Ok(record) => record,
            Err(err) => return Ok(Some(err)),
        };
        let issued = self
            .replay
            .contains(record.key())
            .await
            .map_err(HttpPopError::Backend)?;
        Ok((!issued).then_some(NonceError::NotIssued))
    }

    async fn issue_nonce(&self, nonces: &ServerNonces, jkt: &str) -> Result<ServerNonce, HttpPopError> {
        let issued = nonces
            .mint(&self.resource_identifier, jkt, self.clock.now())
            .map_err(HttpPopError::NonceIssue)?;
        let recorded = self
            .replay
            .check_and_insert(issued.record.key(), nonces.record_ttl_secs(self.freshness.skew()))
            .await
            .map_err(HttpPopError::Backend)?;
        if !recorded {
            return Err(HttpPopError::NonceIssue(NonceIssueError::Collision));
        }
        Ok(issued.nonce)
    }

    async fn verify_presenter(
        &self,
        jwt: &str,
    ) -> Result<(serde_json::Value, String, VerifiedPresenter), HttpPopError> {
        match presenter_kind(jwt)? {
            PresenterKind::Agent => {
                let verified = self
                    .token_verifier
                    .verify_agent(jwt)
                    .await
                    .map_err(HttpPopError::Agent)?;
                let cnf_jwk = verified.claims.cnf.jwk().clone();
                let jkt = verified.jkt.clone();
                Ok((cnf_jwk, jkt, VerifiedPresenter::Agent(verified)))
            }
            PresenterKind::Auth => {
                let verified = self
                    .token_verifier
                    .verify_auth(jwt, &self.resource_identifier)
                    .await
                    .map_err(HttpPopError::Auth)?;
                let cnf = verified.claims.cnf.clone().ok_or(HttpPopError::InvalidConfirmationKey(
                    InvalidConfirmationKeyError::MissingConfirmationClaim,
                ))?;
                let jkt = crate::jkt::jwk_thumbprint(cnf.jwk()).map_err(|e| {
                    HttpPopError::InvalidConfirmationKey(InvalidConfirmationKeyError::StructurallyIncomplete(e))
                })?;
                let presenter = VerifiedPresenter::Auth(VerifiedAuthPresenter {
                    auth: verified,
                    jkt: jkt.clone(),
                });
                Ok((cnf.jwk().clone(), jkt, presenter))
            }
        }
    }
}

/// The record that admits `jti` once for the key with thumbprint `jkt`. The
/// `jti` is digested so a client-chosen value always yields a valid KV key.
fn replay_key(jkt: &str, jti: &Jti) -> String {
    let digest = URL_SAFE_NO_PAD.encode(Sha256::digest(jti.as_str().as_bytes()));
    format!("jti.{jkt}.{digest}")
}

fn ensure_no_duplicate_security_headers(req: &HttpRequest) -> Result<(), HttpPopError> {
    for &name in HTTP_SECURITY_HEADERS {
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

/// Parsed `Signature: sig=:...:` header. An RFC 8941 Byte Sequence is
/// standard base64, while `jsonwebtoken` verifies URL-safe unpadded
/// signatures, so the bytes are decoded in either alphabet and returned in
/// the latter.
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
    let bytes = decode_base64_any_alphabet(inner).ok_or(HttpPopError::MalformedSignature)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
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

/// Checks that the required covered components (#covered-components) and
/// `content-digest` are present in `components`, plus `aauth-mission` and
/// `aauth-nonce` each exactly when the request carries that header.
fn verify_covered_components(req: &HttpRequest, components: &[String]) -> Result<(), HttpPopError> {
    let has = |name: &str| components.iter().any(|c| c == name);

    for required in ["@method", "@authority", "@path", "signature-key"] {
        if !has(required) {
            return Err(HttpPopError::MissingCoveredComponent(required));
        }
    }

    if !has("content-digest") || req.header(headers::CONTENT_DIGEST).is_none() {
        return Err(HttpPopError::MissingContentDigest);
    }

    if has("aauth-mission") != req.header(headers::MISSION).is_some() {
        return Err(HttpPopError::MissingMissionHeader);
    }

    if has(SERVER_NONCE_COMPONENT) != req.header(SERVER_NONCE_HEADER).is_some() {
        return Err(HttpPopError::UnboundNonce);
    }

    Ok(())
}

/// Recomputes SHA-256 over the request body, empty when there is none, and
/// compares it against the `Content-Digest` header.
fn verify_content_digest(req: &HttpRequest) -> Result<(), HttpPopError> {
    let raw = req
        .header(headers::CONTENT_DIGEST)
        .ok_or(HttpPopError::MissingContentDigest)?;
    let supplied = parse_sha256_content_digest(raw).ok_or(HttpPopError::UnsupportedContentDigest)?;
    let expected = Sha256::digest(req.body.as_deref().unwrap_or(&[]));
    if supplied.as_slice() != expected.as_slice() {
        return Err(HttpPopError::ContentDigestMismatch);
    }
    Ok(())
}

/// Extracts the raw `sha-256` digest bytes from an RFC 9530 `Content-Digest`
/// value, a Structured Fields Dictionary of algorithm keys to Byte Sequences
/// (`sha-256=:<base64>:`). Other algorithm entries are skipped rather than
/// rejected, since a peer is free to send additional ones.
///
/// Two RFC 8941 Dictionary rules matter for interop and are honoured here.
/// A member's Item may carry parameters (`sha-256=:...:;q=1`), which are not
/// part of the Byte Sequence and are ignored. A repeated key resolves to its
/// *last* occurrence, so the scan cannot return early: taking the first would
/// make this verifier disagree with any RFC-compliant peer about which digest
/// a duplicated `sha-256` member names.
///
/// Last-wins ranges over the members this reader can read. A `sha-256` member
/// it cannot fails the whole value rather than yielding to a later one, which
/// is what an RFC-compliant peer does too: RFC 8941 discards a field that does
/// not parse, and a discarded `Content-Digest` leaves nothing to check the body
/// against. Skipping ahead would instead let a sender bury a member this
/// verifier cannot read under a trailing one it can.
///
/// So what a member is keyed on, not whether it carries a Byte Sequence,
/// decides between skipping it and failing. RFC 8941 reads a member with no
/// `=` as the Boolean true, which is a `sha-256` naming no digest once it wins
/// last, and a peer is still free to key its own algorithms however it likes.
///
/// This is a targeted reader for one Byte Sequence member, not a general
/// Structured Fields parser. It does not model Inner Lists, and a parameter
/// whose value is a String containing `,` or `;` would split wrongly; no
/// parameter defined for `Content-Digest` takes such a value.
fn parse_sha256_content_digest(raw: &str) -> Option<Vec<u8>> {
    let mut last = None;
    for member in raw.split(',') {
        let (key, value) = match member.split_once('=') {
            Some((key, value)) => (key, Some(value)),
            None => (member, None),
        };
        // A Boolean member's parameters sit on the key, an Item's on its value.
        let key = key.split(';').next().unwrap_or(key);
        if !key.trim().eq_ignore_ascii_case("sha-256") {
            continue;
        }
        // The Byte Sequence ends at its closing colon; anything after that is
        // the parameter list.
        let value = value?.trim_start();
        let inner = value.strip_prefix(':')?.split_once(':')?.0;
        last = Some(decode_base64_any_alphabet(inner)?);
    }
    last
}

/// Decodes a digest that may arrive in any of the base64 alphabets seen in
/// practice. RFC 8941 Byte Sequences are standard padded base64, which is
/// what a conformant third-party client sends, while
/// [`crate::nats_pop::content_digest_sha256`] emits URL-safe unpadded. Both
/// have to decode to the same 32 bytes, so comparison happens on the decoded
/// digest rather than on the header string.
fn decode_base64_any_alphabet(encoded: &str) -> Option<Vec<u8>> {
    STANDARD
        .decode(encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(encoded))
        .or_else(|_| URL_SAFE_NO_PAD.decode(encoded))
        .ok()
}

/// Builds the RFC 9421 canonical signature base for the supported component
/// subset (see module docs). For each covered component identifier, in the
/// order given by `Signature-Input`, appends `"<id>": <value>\n`; then a
/// final `"@signature-params": <inner-list-serialization>` line with no
/// trailing newline, per RFC 9421 Section 2.5.
fn build_signature_base(req: &HttpRequest, input: &SignatureInput) -> Result<String, HttpPopError> {
    let mut base = String::new();
    for component in input.components() {
        let value = component_value(req, component)?;
        base.push('"');
        base.push_str(component);
        base.push_str("\": ");
        base.push_str(&value);
        base.push('\n');
    }
    base.push_str("\"@signature-params\": ");
    base.push_str(&input.serialize());
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
        SERVER_NONCE_COMPONENT => req
            .header(SERVER_NONCE_HEADER)
            .map(normalize_field_value)
            .ok_or(HttpPopError::UnboundNonce),
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

/// The algorithms a `cnf.jwk` can sign a request with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignatureAlgorithm {
    EcdsaP256Sha256,
    EcdsaP384Sha384,
    Ed25519,
}

impl SignatureAlgorithm {
    fn of(jwk: &Jwk) -> Option<Self> {
        match &jwk.algorithm {
            AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P256 => Some(Self::EcdsaP256Sha256),
            AlgorithmParameters::EllipticCurve(ec) if ec.curve == EllipticCurve::P384 => Some(Self::EcdsaP384Sha384),
            AlgorithmParameters::OctetKeyPair(okp) if okp.curve == EllipticCurve::Ed25519 => Some(Self::Ed25519),
            _ => None,
        }
    }

    fn jws(self) -> Algorithm {
        match self {
            Self::EcdsaP256Sha256 => Algorithm::ES256,
            Self::EcdsaP384Sha384 => Algorithm::ES384,
            Self::Ed25519 => Algorithm::EdDSA,
        }
    }

    /// The name RFC 9421 Section 6.2.2 registers, as the `alg` parameter
    /// carries it.
    fn rfc9421_name(self) -> &'static str {
        match self {
            Self::EcdsaP256Sha256 => "ecdsa-p256-sha256",
            Self::EcdsaP384Sha384 => "ecdsa-p384-sha384",
            Self::Ed25519 => "ed25519",
        }
    }
}

fn verify_signature_with_jwk(
    jwk_val: &serde_json::Value,
    declared_alg: Option<&str>,
    base: &[u8],
    sig_b64: &str,
) -> Result<(), HttpPopError> {
    let jwk: Jwk = serde_json::from_value(jwk_val.clone())
        .map_err(|source| HttpPopError::InvalidConfirmationKey(InvalidConfirmationKeyError::Deserialize(source)))?;
    let algorithm = SignatureAlgorithm::of(&jwk).ok_or(HttpPopError::InvalidConfirmationKey(
        InvalidConfirmationKeyError::UnsupportedAlgorithm,
    ))?;
    if declared_alg.is_some_and(|declared| declared != algorithm.rfc9421_name()) {
        return Err(HttpPopError::AlgorithmMismatch);
    }
    let key = DecodingKey::from_jwk(&jwk)
        .map_err(|source| HttpPopError::InvalidConfirmationKey(InvalidConfirmationKeyError::DecodingKey(source)))?;
    let ok = verify(sig_b64, base, &key, algorithm.jws()).map_err(HttpPopError::Verify)?;
    if !ok {
        return Err(HttpPopError::BadSignature);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
