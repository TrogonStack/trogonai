//! AAuth (draft-hardt-aauth-protocol) ingress for the A2A gateway.
//!
//! Verifies inline `aa-agent+jwt` + PoP headers carried on the ingress NATS
//! message and, when present, an `aa-auth+jwt` token. On enforce-mode failure
//! the module emits a deny carrying an `AAuth-Requirement` challenge for the
//! reply header. Wiring into `runtime::dispatch_gateway_ingress` lands in a
//! later slice — this slice exposes the verifier capability independently of
//! the dispatch glue so it can be unit-tested in isolation.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use jsonwebtoken::jwk::JwkSet;
use trogon_aauth_verify::challenge::ResourceChallenge;
use trogon_aauth_verify::jwks::JwksError;
use trogon_aauth_verify::mission::MissionError;
use trogon_aauth_verify::nats_pop::{NatsHeaders, NatsPopError, NatsRequest};
use trogon_aauth_verify::{
    CachedJwksResolver, ChallengeMinter, HttpJwksResolver, InMemoryReplayStore, JwksResolver, NatsPopVerifier,
    ReplayStore, SystemTimeSource, TokenError, TokenVerifier, extract_mission_claim,
    verify_mission_header_matches_claim,
};
use trogon_identity_types::aauth::headers::Capabilities;
use trogon_identity_types::aauth::mission::MissionHeader;
use trogon_identity_types::aauth::{MissionRef, headers as aauth_headers};

/// Anti-replay code returned on enforce-mode AAuth failure. JSON-RPC clients
/// see this code; A2A ingress callers see a status reply carrying the
/// `AAuth-Requirement` header.
pub const AAUTH_REQUIRED_CODE: i32 = -32_118;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AAuthMode {
    Off,
    Shadow,
    Enforce,
}

/// Validated non-empty, trimmed string for the resource issuer claim
/// (`iss`) the gateway mints into challenge tokens. Carrying validation in
/// the type stops downstream code from inheriting empty primitives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceIssuer(String);

impl ResourceIssuer {
    pub fn new(raw: impl Into<String>) -> Result<Self, ResourceIssuerError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ResourceIssuerError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResourceIssuerError {
    #[error("resource issuer must not be empty")]
    Empty,
}

/// Validated Person-Server audience the challenge token addresses
/// (`aud_ps`). Non-empty, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonServerAudience(String);

impl PersonServerAudience {
    pub fn new(raw: impl Into<String>) -> Result<Self, PersonServerAudienceError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(PersonServerAudienceError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PersonServerAudienceError {
    #[error("person-server audience must not be empty")]
    Empty,
}

/// Validated `kid` the gateway's challenge minter uses. Non-empty, trimmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeKid(String);

impl ChallengeKid {
    pub fn new(raw: impl Into<String>) -> Result<Self, ChallengeKidError> {
        let value = raw.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(ChallengeKidError::Empty);
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ChallengeKidError {
    #[error("challenge kid must not be empty")]
    Empty,
}

/// JWT verifier leeway in seconds, applied to both the PoP path and the
/// `aa-auth+jwt` path so non-default leeway values produce consistent agent
/// vs auth clock tolerance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeewaySecs(u64);

impl LeewaySecs {
    #[must_use]
    pub fn new(secs: u64) -> Self {
        Self(secs)
    }

    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Non-negative second count. PoP skew and challenge TTL share this shape;
/// the constructor refuses negative i64 because the verifier code path uses
/// `unsigned_abs` and a negative input would silently parse as a huge
/// positive window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonNegativeSecs(i64);

impl NonNegativeSecs {
    pub fn new(secs: i64) -> Result<Self, NonNegativeSecsError> {
        if secs < 0 {
            return Err(NonNegativeSecsError::Negative(secs));
        }
        Ok(Self(secs))
    }

    #[must_use]
    pub fn get(self) -> i64 {
        self.0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NonNegativeSecsError {
    #[error("expected non-negative seconds, got {0}")]
    Negative(i64),
}

/// The gateway's resource-defined scope grammar (draft "Scopes" -- scope
/// values are resource-specific and the resource publishes their meaning).
///
/// A scope string is a space-separated list of method-glob tokens matched
/// against the invoked method's dotted form (e.g. `message.send`,
/// `tasks.get`). A token is one of:
///
/// - `*` -- covers every method.
/// - `{prefix}.*` -- covers every method dotted-prefixed by `{prefix}.`
///   (e.g. `message.*` covers `message.send` and `message.stream`, but not
///   bare `message`).
/// - an exact dotted method name -- covers only that method.
///
/// Matching is case-sensitive and requires an exact segment boundary at the
/// `.` so `message.*` cannot accidentally match `messages.send`. An empty or
/// all-whitespace scope string covers nothing.
#[must_use]
pub fn scope_covers_method(scope: &str, method_dots: &str) -> bool {
    scope
        .split_whitespace()
        .any(|token| scope_token_covers(token, method_dots))
}

fn scope_token_covers(token: &str, method_dots: &str) -> bool {
    if token == "*" {
        return true;
    }
    if let Some(prefix) = token.strip_suffix(".*") {
        return !prefix.is_empty()
            && method_dots
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('.'));
    }
    token == method_dots
}

pub struct AAuthConfig<R: JwksResolver> {
    pub mode: AAuthMode,
    pub jwks: R,
    pub resource_iss: ResourceIssuer,
    pub person_server_aud: PersonServerAudience,
    pub leeway_secs: LeewaySecs,
    pub challenge_alg: jsonwebtoken::Algorithm,
    pub challenge_key: jsonwebtoken::EncodingKey,
    pub challenge_kid: ChallengeKid,
    pub challenge_ttl_secs: NonNegativeSecs,
    pub max_skew_secs: NonNegativeSecs,
}

pub struct AAuthIngress<R: JwksResolver + Clone, S: ReplayStore> {
    pub mode: AAuthMode,
    pop_verifier: NatsPopVerifier<R, SystemTimeSource, S>,
    token_verifier: TokenVerifier<R, SystemTimeSource>,
    challenge: ChallengeMinter<SystemTimeSource>,
    cfg: AAuthRuntime,
}

#[derive(Clone, Debug)]
struct AAuthRuntime {
    resource_iss: ResourceIssuer,
    person_server_aud: PersonServerAudience,
    challenge_kid: ChallengeKid,
    challenge_ttl_secs: NonNegativeSecs,
}

impl<R: JwksResolver + Clone + 'static> AAuthIngress<R, InMemoryReplayStore> {
    pub fn new_in_memory(cfg: AAuthConfig<R>) -> Self {
        Self::with_replay(cfg, InMemoryReplayStore::new())
    }
}

impl<R: JwksResolver + Clone + 'static, S: ReplayStore> AAuthIngress<R, S> {
    pub fn with_replay(cfg: AAuthConfig<R>, replay: S) -> Self {
        let leeway = cfg.leeway_secs.get();
        let token_verifier = TokenVerifier::new(cfg.jwks.clone(), SystemTimeSource).with_leeway(leeway);
        // The PoP verifier carries its own embedded `TokenVerifier` for the
        // `aa-agent+jwt` it parses out of headers. Without explicitly
        // applying our configured leeway to that inner verifier, a
        // non-default leeway would apply only to `aa-auth+jwt` checks and
        // agent token freshness would be measured against `TokenVerifier`'s
        // library default (60s), producing inconsistent agent-vs-auth
        // clock tolerance.
        let pop_inner_jwks = cfg.jwks.clone();
        let mut pop = NatsPopVerifier::new(cfg.jwks, SystemTimeSource, replay);
        pop.max_skew_secs = cfg.max_skew_secs.get();
        pop.token_verifier = TokenVerifier::new(pop_inner_jwks, SystemTimeSource).with_leeway(leeway);
        let challenge = ChallengeMinter::new(cfg.challenge_key, cfg.challenge_alg, SystemTimeSource);
        Self {
            mode: cfg.mode,
            pop_verifier: pop,
            token_verifier,
            challenge,
            cfg: AAuthRuntime {
                resource_iss: cfg.resource_iss,
                person_server_aud: cfg.person_server_aud,
                challenge_kid: cfg.challenge_kid,
                challenge_ttl_secs: cfg.challenge_ttl_secs,
            },
        }
    }

    /// Verify the inline `aa-agent+jwt` + PoP envelope and optional
    /// `aa-auth+jwt` carried on a NATS ingress message, then apply the
    /// resource-defined enforcement the draft delegates to us: `scope`
    /// coverage of `method_dots` (draft "Scopes") and `AAuth-Mission`
    /// header-vs-claim consistency (draft "Mission Context at Resources").
    ///
    /// `method_dots` is the dotted method the caller is invoking (e.g.
    /// `message.send`); it is only consulted for scope enforcement when an
    /// auth token is present, so callers that haven't yet resolved a method
    /// (there are none in the gateway today) may pass `""`.
    pub async fn resolve_nats(
        &self,
        subject: &str,
        reply: Option<&str>,
        payload: &[u8],
        headers: &[(String, String)],
        auth_token: Option<&str>,
        method_dots: &str,
    ) -> Result<AAuthResolution, AAuthDeny> {
        if self.mode == AAuthMode::Off {
            return Ok(AAuthResolution::anonymous());
        }

        let req = NatsRequest {
            subject,
            reply,
            payload,
            headers: NatsHeaders::new(headers),
        };
        let agent = match self.pop_verifier.verify(&req).await {
            Ok(a) => a,
            Err(e) => return self.deny_or_shadow(AAuthDenyReason::Pop(e), None).await,
        };

        let mut resolution = AAuthResolution::from_agent(&agent);
        resolution.capabilities = header_value(headers, aauth_headers::CAPABILITIES).map(Capabilities::parse);
        let mission_header = header_value(headers, aauth_headers::MISSION).and_then(MissionHeader::parse);

        if let Some(auth_jwt) = auth_token {
            let auth = match self
                .token_verifier
                .verify_auth(auth_jwt, self.cfg.resource_iss.as_str())
                .await
            {
                Ok(auth) => auth,
                Err(e) => {
                    return self
                        .deny_or_shadow(AAuthDenyReason::Auth(e), Some(ChallengeBinding::from_agent(&agent)))
                        .await;
                }
            };
            // Auth token must bind to the PoP-verified agent. Otherwise a
            // caller could pair its own valid `aa-agent+jwt` + PoP with
            // someone else's `aa-auth+jwt` and inherit the foreign
            // principal. Refuse when either side disagrees.
            if auth.claims.agent != agent.claims.sub || auth.claims.agent_jkt != agent.jkt {
                return self
                    .deny_or_shadow(
                        AAuthDenyReason::AuthAgentMismatch {
                            agent_sub: agent.claims.sub.clone(),
                            agent_jkt: agent.jkt.clone(),
                            auth_agent: auth.claims.agent.clone(),
                            auth_agent_jkt: auth.claims.agent_jkt.clone(),
                        },
                        Some(ChallengeBinding::from_agent(&agent)),
                    )
                    .await;
            }

            // Scopes (draft "Scopes"): an uncovered method means the agent
            // needs a broader auth token, not a fresh PoP/agent-token
            // exchange -- mint the same `requirement=auth-token` challenge
            // the auth-token-required path uses.
            if !method_dots.is_empty() && !scope_covers_method(&auth.claims.scope, method_dots) {
                let reason = AAuthDenyReason::ScopeNotCovered {
                    scope: auth.claims.scope.clone(),
                    method: method_dots.to_owned(),
                };
                return self
                    .deny_or_shadow(reason, Some(ChallengeBinding::from_agent(&agent)))
                    .await;
            }

            // Mission (draft "Mission Context at Resources"): a mission
            // claim on the auth token binds the grant to its mission, so a
            // malformed claim and a claim presented without the matching
            // `AAuth-Mission` header are both denials, not skipped checks --
            // silently ignoring either would let a mission-scoped token act
            // as an unscoped one.
            let claims_raw = raw_claims_value(&auth.raw_jwt);
            let mission_claim = match claims_raw.as_ref().map(extract_mission_claim).transpose() {
                Ok(claim) => claim.flatten(),
                Err(e) => {
                    return self
                        .deny_or_shadow(
                            AAuthDenyReason::MissionMismatch(e),
                            Some(ChallengeBinding::from_agent(&agent)),
                        )
                        .await;
                }
            };
            match (mission_header.as_ref(), mission_claim.as_ref()) {
                (Some(header), Some(claim)) => {
                    let header_ref = MissionRef {
                        approver: header.approver.clone(),
                        s256: header.s256.clone(),
                    };
                    if let Err(e) = verify_mission_header_matches_claim(&header_ref, claim) {
                        return self
                            .deny_or_shadow(
                                AAuthDenyReason::MissionMismatch(e),
                                Some(ChallengeBinding::from_agent(&agent)),
                            )
                            .await;
                    }
                    resolution.mission_approver = Some(claim.approver.clone());
                }
                (None, Some(claim)) => {
                    return self
                        .deny_or_shadow(
                            AAuthDenyReason::MissionHeaderMissing {
                                approver: claim.approver.clone(),
                            },
                            Some(ChallengeBinding::from_agent(&agent)),
                        )
                        .await;
                }
                (Some(_) | None, None) => {}
            }

            resolution.attach_auth(auth);
        }

        Ok(resolution)
    }

    async fn deny_or_shadow(
        &self,
        reason: AAuthDenyReason,
        binding: Option<ChallengeBinding<'_>>,
    ) -> Result<AAuthResolution, AAuthDeny> {
        if self.mode == AAuthMode::Shadow {
            // The reason is logged with %{reason} so the typed source chain
            // reaches operators without coupling them to its Display string.
            tracing::warn!(event = "aauth.shadow_deny", reason = %reason);
            return Ok(AAuthResolution::anonymous());
        }
        let challenge = binding.and_then(|b| {
            let jti = uuid_like();
            self.challenge
                .mint(&ResourceChallenge {
                    iss: self.cfg.resource_iss.as_str(),
                    aud_ps: self.cfg.person_server_aud.as_str(),
                    // Bind the challenge to the PoP-verified agent so the
                    // Person Server exchange can be tied to the presenting
                    // agent, not just its key.
                    agent: b.agent_sub,
                    agent_jkt: b.agent_jkt,
                    scope: "*",
                    ttl_secs: self.cfg.challenge_ttl_secs.get(),
                    kid: self.cfg.challenge_kid.as_str(),
                    jti: &jti,
                    mission: None as Option<MissionRef>,
                })
                .ok()
        });
        Err(AAuthDeny {
            code: AAUTH_REQUIRED_CODE,
            reason,
            challenge,
        })
    }
}

/// Pair of `sub` / `jkt` the verifier already authenticated on a presenting
/// agent. Used to bind a minted `aa-resource+jwt` challenge to that agent.
struct ChallengeBinding<'a> {
    agent_sub: &'a str,
    agent_jkt: &'a str,
}

impl<'a> ChallengeBinding<'a> {
    fn from_agent(agent: &'a trogon_aauth_verify::VerifiedAgent) -> Self {
        Self {
            agent_sub: &agent.claims.sub,
            agent_jkt: &agent.jkt,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AAuthResolution {
    pub agent_id: Option<String>,
    pub agent_jkt: Option<String>,
    pub agent_iss: Option<String>,
    pub agent_jti: Option<String>,
    pub auth_jti: Option<String>,
    pub principal: Option<String>,
    /// Parsed `AAuth-Capabilities` request header (draft "AAuth-Capabilities
    /// Request Header"), when the agent sent one. `None` means the header was
    /// absent -- per spec, recipients MUST NOT assume any capabilities in
    /// that case, so callers must not default this to an empty-but-present
    /// list.
    pub capabilities: Option<Capabilities>,
    /// The auth token's `scope` claim (draft "Scopes"), when an `aa-auth+jwt`
    /// was presented and verified. `None` when no auth token was presented --
    /// scope enforcement does not apply to agent-only access.
    pub scope: Option<String>,
    /// `approver` from a verified `AAuth-Mission` header / auth token mission
    /// claim match (draft "Mission Context at Resources"), recorded for the
    /// audit trail once header-vs-claim consistency has been checked.
    pub mission_approver: Option<String>,
}

impl AAuthResolution {
    fn anonymous() -> Self {
        Self {
            agent_id: None,
            agent_jkt: None,
            agent_iss: None,
            agent_jti: None,
            auth_jti: None,
            principal: None,
            capabilities: None,
            scope: None,
            mission_approver: None,
        }
    }

    fn from_agent(agent: &trogon_aauth_verify::VerifiedAgent) -> Self {
        Self {
            agent_id: Some(agent.claims.sub.clone()),
            agent_jkt: Some(agent.jkt.clone()),
            agent_iss: Some(agent.claims.iss.clone()),
            agent_jti: Some(agent.claims.jti.clone()),
            auth_jti: None,
            principal: None,
            capabilities: None,
            scope: None,
            mission_approver: None,
        }
    }

    fn attach_auth(&mut self, auth: trogon_aauth_verify::VerifiedAuth) {
        self.auth_jti = Some(auth.claims.jti.clone());
        self.principal = auth.claims.principal.clone();
        self.scope = Some(auth.claims.scope.clone());
    }
}

/// Why AAuth refused a request. Carries the typed source from the underlying
/// verifier rather than a stringified message so callers can pattern-match
/// the failure mode (PoP vs. auth token) without parsing display text.
#[derive(Debug, thiserror::Error)]
pub enum AAuthDenyReason {
    #[error("nats-pop verification: {0}")]
    Pop(#[source] NatsPopError),
    #[error("aa-auth+jwt verification: {0}")]
    Auth(#[source] TokenError),
    /// The `aa-auth+jwt` verified, but its `agent` / `agent_jkt` claims do
    /// not match the PoP-verified agent. Surface it as a distinct variant so
    /// audit can record agent-impersonation attempts separately from token
    /// validation failures.
    #[error(
        "aa-auth+jwt does not bind to presenting agent (auth.agent={auth_agent:?} \
         auth.agent_jkt={auth_agent_jkt:?} vs presenter sub={agent_sub:?} jkt={agent_jkt:?})"
    )]
    AuthAgentMismatch {
        agent_sub: String,
        agent_jkt: String,
        auth_agent: String,
        auth_agent_jkt: String,
    },
    /// The verified `aa-auth+jwt`'s `scope` claim does not cover the invoked
    /// method under the gateway's scope grammar (see [`scope_covers_method`]).
    /// Distinct from [`AAuthDenyReason::Auth`] because the token itself is
    /// valid -- it simply doesn't authorize this method, so the agent needs a
    /// step-up auth token rather than a fresh PoP/agent-token exchange.
    #[error("aa-auth+jwt scope {scope:?} does not cover method {method:?}")]
    ScopeNotCovered { scope: String, method: String },
    /// The request's `AAuth-Mission` header did not match the verified
    /// `aa-auth+jwt`'s `mission` claim (draft "Mission Context at Resources",
    /// "Request-Context Binding" rule 9).
    #[error("aauth-mission header does not match auth token mission claim: {0}")]
    MissionMismatch(#[source] MissionError),
    /// The verified `aa-auth+jwt` carries a `mission` claim but the request
    /// has no parseable `AAuth-Mission` header. A mission-scoped grant is
    /// only valid inside its mission context, so the binding check cannot be
    /// skipped just because the presenter omitted the header.
    #[error("aa-auth+jwt is mission-scoped (approver {approver:?}) but the request carries no AAuth-Mission header")]
    MissionHeaderMissing { approver: String },
}

#[derive(Debug, thiserror::Error)]
#[error("aauth denied: {reason}")]
pub struct AAuthDeny {
    pub code: i32,
    #[source]
    pub reason: AAuthDenyReason,
    /// Minted `aa-resource+jwt` challenge token that the reply must carry as
    /// the `AAuth-Requirement` header. `None` when challenge minting failed
    /// (e.g. agent didn't present a verifiable `cnf.jwk`).
    pub challenge: Option<String>,
}

impl AAuthDeny {
    #[must_use]
    pub fn to_requirement_header(&self) -> Option<(String, String)> {
        self.challenge.as_ref().map(|tok| {
            (
                aauth_headers::REQUIREMENT.to_string(),
                format!("requirement=auth-token; resource-token=\"{tok}\""),
            )
        })
    }
}

fn uuid_like() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("jti-{nanos:x}")
}

/// Case-insensitive lookup of a NATS header pair by name. `NatsHeaders`
/// (the PoP verifier's own header view) does not expose a public getter, so
/// callers that need to read an AAuth header outside the PoP envelope (e.g.
/// `AAuth-Capabilities`, `AAuth-Mission`) look the raw pairs up directly.
fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Decode a JWT's payload segment into raw claims JSON without verifying the
/// signature -- the token has already been signature-and-freshness verified
/// by [`TokenVerifier::verify_auth`] by the time this runs; this only exists
/// because `AuthClaims` has no typed `mission` field (see
/// `trogon_aauth_verify::mission` module docs), so the `mission` claim must
/// be read off the raw payload via `extract_mission_claim`.
fn raw_claims_value(jwt: &str) -> Option<serde_json::Value> {
    use base64::Engine;
    let payload_b64 = jwt.split('.').nth(1)?;
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64.as_bytes())
        .ok()?;
    serde_json::from_slice(&payload).ok()
}

pub use trogon_aauth_verify::StaticJwks;

pub type DefaultAAuthIngress<R> = Arc<AAuthIngress<R, InMemoryReplayStore>>;

/// The gateway's JWKS resolution strategy, selected at boot from
/// `A2A_GATEWAY_AAUTH_JWKS_PATH` (static) or
/// `A2A_GATEWAY_AAUTH_JWKS_DISCOVERY` (HTTP well-known, TTL-cached).
///
/// `CachedJwksResolver` has no `Clone` impl of its own (it owns interior
/// mutable cache state behind an `RwLock`), so the discovery variant wraps it
/// in an `Arc` -- cheap to clone and consistent with how `AAuthIngress`
/// already clones its `JwksResolver` into each of its sub-verifiers.
#[derive(Clone)]
pub enum GatewayJwksResolver {
    Static(StaticJwks),
    WellKnown(Arc<CachedJwksResolver<HttpJwksResolver, SystemTimeSource>>),
}

#[async_trait]
impl JwksResolver for GatewayJwksResolver {
    async fn resolve(&self, iss: &str) -> Result<JwkSet, JwksError> {
        match self {
            GatewayJwksResolver::Static(jwks) => jwks.resolve(iss).await,
            GatewayJwksResolver::WellKnown(cached) => cached.resolve(iss).await,
        }
    }
}

/// The gateway's concrete AAuth ingress type. Parameterized over
/// [`GatewayJwksResolver`] so a single ingress layer supports either a
/// static JWKS file or HTTP well-known discovery, chosen at boot from
/// environment (see `runtime::aauth_env`).
pub type GatewayAAuthIngress = DefaultAAuthIngress<GatewayJwksResolver>;

#[cfg(test)]
mod tests;
