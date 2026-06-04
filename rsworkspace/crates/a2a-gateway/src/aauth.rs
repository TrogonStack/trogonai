//! AAuth (draft-hardt-aauth-protocol) ingress for the A2A gateway.
//!
//! Verifies inline `aa-agent+jwt` + PoP headers carried on the ingress NATS
//! message and, when present, an `aa-auth+jwt` token. On enforce-mode failure,
//! emits a deny carrying an `AAuth-Requirement` challenge for the reply header.
//!
//! Mirrors `trogon_mcp_gateway::aauth` so the two ingress paths share a single
//! verifier model. Wiring into `runtime::dispatch_gateway_ingress` lands when
//! the auth-callout vs. inline header decision is finalized in policy config;
//! the module exposes the verifier capability independently of that wiring.

use std::sync::Arc;

use trogon_aauth_verify::challenge::ResourceChallenge;
use trogon_aauth_verify::nats_pop::{NatsHeaders, NatsRequest};
use trogon_aauth_verify::{
    ChallengeMinter, InMemoryReplayStore, JwksResolver, NatsPopVerifier, ReplayStore, SystemTimeSource, TokenVerifier,
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
            Err(e) => return self.deny_or_shadow(e.to_string(), None).await,
        };

        let mut resolution = AAuthResolution::from_agent(&agent);

        if let Some(auth_jwt) = auth_token {
            match self.token_verifier.verify_auth(auth_jwt, &self.cfg.resource_iss).await {
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
                .mint(&ResourceChallenge {
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
            code: AAUTH_REQUIRED_CODE,
            message: reason,
            challenge,
        })
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

#[derive(Debug, Clone)]
pub struct AAuthDeny {
    pub code: i32,
    pub message: String,
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
mod tests {
    use super::*;

    #[test]
    fn deny_renders_requirement_header_when_challenge_present() {
        let deny = AAuthDeny {
            code: AAUTH_REQUIRED_CODE,
            message: "bad jwt".into(),
            challenge: Some("tok123".into()),
        };
        let (name, value) = deny.to_requirement_header().expect("header");
        assert_eq!(name, aauth_headers::REQUIREMENT);
        assert!(value.contains("tok123"));
        assert!(value.starts_with("requirement=auth-token"));
    }

    #[test]
    fn deny_without_challenge_has_no_header() {
        let deny = AAuthDeny {
            code: AAUTH_REQUIRED_CODE,
            message: "bad jwt".into(),
            challenge: None,
        };
        assert!(deny.to_requirement_header().is_none());
    }

    #[test]
    fn resolution_anonymous_has_no_identity() {
        let res = AAuthResolution::anonymous();
        assert!(res.agent_id.is_none());
        assert!(res.agent_jkt.is_none());
        assert!(res.principal.is_none());
    }
}
