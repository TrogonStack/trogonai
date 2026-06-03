//! AAuth (draft-hardt-aauth-protocol) ingress for the MCP gateway.
//!
//! Provides a verifier wrapper that the gateway can call on inbound HTTP and
//! NATS requests. The verifier validates `aa-agent+jwt` + PoP, optionally
//! requires an `aa-auth+jwt`, and mints `aa-resource+jwt` challenges on deny.
//!
//! Composes with the existing `jwt.rs` ingress: `aauth.rs` operates on the same
//! `GatewayIdentity` / `IdentityDeny` shapes so policy code is unchanged.

use std::sync::Arc;

use jsonwebtoken::{Algorithm, EncodingKey};
use trogon_aauth_verify::nats_pop::{NatsHeaders, NatsRequest};
use trogon_aauth_verify::{
    ChallengeMinter, InMemoryReplayStore, JwksResolver, NatsPopVerifier, ReplayStore, SystemTimeSource, TokenVerifier,
};
use trogon_identity_types::aauth::{MissionRef, headers as aauth_headers};

use crate::authz::{GatewayIdentity, IdentitySource};
use crate::jwt::IdentityDeny;
use crate::rpc_codes;

/// Mode for AAuth ingress. Mirrors `JwtMode` semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AAuthMode {
    Off,
    Shadow,
    Enforce,
}

/// Gateway-level AAuth configuration.
pub struct AAuthConfig<R: JwksResolver> {
    pub mode: AAuthMode,
    pub jwks: R,
    pub resource_iss: String,
    pub person_server_aud: String,
    pub leeway_secs: u64,
    pub challenge_alg: Algorithm,
    pub challenge_key: EncodingKey,
    pub challenge_kid: String,
    pub challenge_ttl_secs: i64,
    pub max_skew_secs: i64,
}

/// AAuth verifier handle used by the gateway ingress.
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
    /// Build an ingress backed by in-memory replay protection. Single-process
    /// gateways use this; multi-replica deployments should pass a NATS KV store.
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

    /// Resolve a NATS request to a `GatewayIdentity`. On enforce-mode failure,
    /// returns an `IdentityDeny` carrying an `AAuth-Requirement` challenge
    /// suitable for the reply header. In shadow mode, failures are downgraded.
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
            Err(e) => return self.deny_or_shadow(e.to_string(), None).await,
        };

        let mut resolution = AAuthResolution::from_agent(&agent);

        if let Some(auth_jwt) = auth_token {
            match self
                .token_verifier
                .verify_auth(auth_jwt, &self.cfg.resource_iss)
                .await
            {
                Ok(auth) => resolution.attach_auth(auth),
                Err(e) => return self.deny_or_shadow(e.to_string(), Some(&agent.jkt)).await,
            }
        }

        Ok(resolution)
    }

    async fn deny_or_shadow(&self, reason: String, jkt: Option<&str>) -> Result<AAuthResolution, AAuthDeny> {
        if self.mode == AAuthMode::Shadow {
            tracing::warn!(event = "aauth.shadow_deny", reason);
            return Ok(AAuthResolution::anonymous());
        }
        let challenge = jkt.and_then(|jkt| {
            let jti = uuid_like();
            self.challenge
                .mint(&trogon_aauth_verify::challenge::ResourceChallenge {
                    iss: &self.cfg.resource_iss,
                    aud_ps: &self.cfg.person_server_aud,
                    agent: "",
                    agent_jkt: jkt,
                    scope: "*",
                    ttl_secs: self.cfg.challenge_ttl_secs,
                    kid: &self.cfg.challenge_kid,
                    jti: &jti,
                    mission: None as Option<MissionRef>,
                })
                .ok()
        });
        Err(AAuthDeny {
            code: rpc_codes::AUTH_REQUIRED,
            message: reason,
            challenge,
        })
    }
}

#[derive(Clone, Debug)]
pub struct AAuthResolution {
    pub identity: GatewayIdentity,
    pub agent_id: Option<String>,
    pub agent_jkt: Option<String>,
    pub auth_jti: Option<String>,
    pub principal: Option<String>,
}

impl AAuthResolution {
    fn anonymous() -> Self {
        Self {
            identity: GatewayIdentity {
                tenant: None,
                caller_sub: None,
                issuer: None,
                jti: None,
                source: IdentitySource::Anonymous,
            },
            agent_id: None,
            agent_jkt: None,
            auth_jti: None,
            principal: None,
        }
    }

    fn from_agent(agent: &trogon_aauth_verify::VerifiedAgent) -> Self {
        Self {
            identity: GatewayIdentity {
                tenant: None,
                caller_sub: Some(agent.claims.sub.clone()),
                issuer: Some(agent.claims.iss.clone()),
                jti: Some(agent.claims.jti.clone()),
                source: IdentitySource::Jwt,
            },
            agent_id: Some(agent.claims.sub.clone()),
            agent_jkt: Some(agent.jkt.clone()),
            auth_jti: None,
            principal: None,
        }
    }

    fn attach_auth(&mut self, auth: trogon_aauth_verify::VerifiedAuth) {
        self.auth_jti = Some(auth.claims.jti.clone());
        self.principal = auth.claims.principal.clone();
        // The principal becomes the caller_sub when present.
        if let Some(p) = &self.principal {
            self.identity.caller_sub = Some(p.clone());
        }
    }
}

#[derive(Debug, Clone)]
pub struct AAuthDeny {
    pub code: i32,
    pub message: String,
    /// Wire value for the `AAuth-Requirement` reply header.
    pub challenge: Option<String>,
}

impl AAuthDeny {
    /// Render the `AAuth-Requirement` header value when a challenge token was minted.
    #[must_use]
    pub fn to_requirement_header(&self) -> Option<(String, String)> {
        self.challenge.as_ref().map(|tok| {
            (
                aauth_headers::REQUIREMENT.to_string(),
                format!("requirement=auth-token; resource-token=\"{tok}\""),
            )
        })
    }

    #[must_use]
    pub fn into_identity_deny(self) -> IdentityDeny {
        IdentityDeny {
            code: self.code,
            message: self.message,
        }
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

/// Re-export so callers don't need a direct dep on trogon-aauth-verify.
pub use trogon_aauth_verify::StaticJwks;

/// Convenience alias for tests / single-process deployments.
pub type DefaultAAuthIngress<R> = Arc<AAuthIngress<R, InMemoryReplayStore>>;
