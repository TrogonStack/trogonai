//! AAuth (draft-hardt-aauth-protocol) ingress for the A2A gateway.
//!
//! Verifies inline `aa-agent+jwt` + PoP headers carried on the ingress NATS
//! message and, when present, an `aa-auth+jwt` token. On enforce-mode failure
//! the module emits a deny carrying an `AAuth-Requirement` challenge for the
//! reply header. Wiring into `runtime::dispatch_gateway_ingress` lands in a
//! later slice — this slice exposes the verifier capability independently of
//! the dispatch glue so it can be unit-tested in isolation.

use std::sync::Arc;

use trogon_aauth_verify::challenge::ResourceChallenge;
use trogon_aauth_verify::nats_pop::{NatsHeaders, NatsPopError, NatsRequest};
use trogon_aauth_verify::{
    ChallengeMinter, InMemoryReplayStore, JwksResolver, NatsPopVerifier, ReplayStore, SystemTimeSource, TokenError,
    TokenVerifier,
};
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

pub struct AAuthConfig<R: JwksResolver> {
    pub mode: AAuthMode,
    pub jwks: R,
    pub resource_iss: String,
    pub person_server_aud: String,
    pub leeway_secs: u64,
    pub challenge_alg: jsonwebtoken::Algorithm,
    pub challenge_key: jsonwebtoken::EncodingKey,
    pub challenge_kid: String,
    pub challenge_ttl_secs: i64,
    pub max_skew_secs: i64,
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
    resource_iss: String,
    person_server_aud: String,
    challenge_kid: String,
    challenge_ttl_secs: i64,
}

impl<R: JwksResolver + Clone + 'static> AAuthIngress<R, InMemoryReplayStore> {
    pub fn new_in_memory(cfg: AAuthConfig<R>) -> Self {
        Self::with_replay(cfg, InMemoryReplayStore::new())
    }
}

impl<R: JwksResolver + Clone + 'static, S: ReplayStore> AAuthIngress<R, S> {
    pub fn with_replay(cfg: AAuthConfig<R>, replay: S) -> Self {
        let token_verifier = TokenVerifier::new(cfg.jwks.clone(), SystemTimeSource).with_leeway(cfg.leeway_secs);
        let mut pop = NatsPopVerifier::new(cfg.jwks, SystemTimeSource, replay);
        pop.max_skew_secs = cfg.max_skew_secs;
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

    pub async fn resolve_nats(
        &self,
        subject: &str,
        reply: Option<&str>,
        payload: &[u8],
        headers: &[(String, String)],
        auth_token: Option<&str>,
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

        if let Some(auth_jwt) = auth_token {
            let auth = match self.token_verifier.verify_auth(auth_jwt, &self.cfg.resource_iss).await {
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
                    iss: &self.cfg.resource_iss,
                    aud_ps: &self.cfg.person_server_aud,
                    // Bind the challenge to the PoP-verified agent so the
                    // Person Server exchange can be tied to the presenting
                    // agent, not just its key.
                    agent: b.agent_sub,
                    agent_jkt: b.agent_jkt,
                    scope: "*",
                    ttl_secs: self.cfg.challenge_ttl_secs,
                    kid: &self.cfg.challenge_kid,
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
        }
    }

    fn attach_auth(&mut self, auth: trogon_aauth_verify::VerifiedAuth) {
        self.auth_jti = Some(auth.claims.jti.clone());
        self.principal = auth.claims.principal.clone();
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
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("jti-{nanos:x}")
}

pub use trogon_aauth_verify::StaticJwks;

pub type DefaultAAuthIngress<R> = Arc<AAuthIngress<R, InMemoryReplayStore>>;

#[cfg(test)]
mod tests;
