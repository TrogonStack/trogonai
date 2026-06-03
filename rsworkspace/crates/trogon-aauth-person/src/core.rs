//! Person Server core logic. Transport-agnostic; HTTP and NATS adapters call
//! into these methods.

use std::sync::Arc;

use jsonwebtoken::{Algorithm, EncodingKey, Header, encode, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use trogon_aauth_verify::{JwksResolver, TokenVerifier, jwk_thumbprint};
use trogon_aauth_verify::time_source::TimeSource;
use trogon_identity_types::aauth::{DWK_AGENT, TYP_AGENT, TYP_AUTH};

use crate::policy::{ConsentContext, ConsentDecision, ConsentPolicy};
use crate::store::{AgentRecord, ConsentRecord, PersonStore};

#[derive(Debug, thiserror::Error)]
pub enum PersonError {
    #[error("invalid request: {0}")]
    BadRequest(String),
    #[error("resource token verification failed: {0}")]
    ResourceTokenInvalid(String),
    #[error("consent denied: {0}")]
    ConsentDenied(String),
    #[error("requires interaction at {url}")]
    RequiresInteraction { url: String, code: Option<String> },
    #[error("store: {0}")]
    Store(String),
    #[error("encode: {0}")]
    Encode(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapRequest {
    /// The agent's public JWK (the `cnf.jwk`).
    pub cnf_jwk: serde_json::Value,
    /// The principal this agent acts on behalf of (may be empty for service agents).
    #[serde(default)]
    pub principal: Option<String>,
    /// Optional agent-supplied id; if empty the PS generates one.
    #[serde(default)]
    pub agent_id_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapResponse {
    pub agent_jwt: String,
    pub agent_id: String,
    pub expires_in: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenRequest {
    /// `aa-resource+jwt` returned from the resource as a 401 challenge.
    pub resource_jwt: String,
    /// The agent's identity token (must match `agent_jkt` in the resource token).
    pub agent_jwt: String,
    /// Principal on whose behalf the exchange is requested.
    #[serde(default)]
    pub principal: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub auth_jwt: String,
    pub scope: String,
    pub expires_in: i64,
}

pub struct PersonCore<R: JwksResolver, S: PersonStore, P: ConsentPolicy, C: TimeSource> {
    pub iss: String,
    pub signing_kid: String,
    pub signing_alg: Algorithm,
    pub signing_key: EncodingKey,
    pub agent_jwt_ttl_secs: i64,
    pub auth_jwt_ttl_secs: i64,
    pub store: Arc<S>,
    pub policy: Arc<P>,
    pub token_verifier: TokenVerifier<R, C>,
    pub clock: C,
}

impl<R, S, P, C> PersonCore<R, S, P, C>
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    pub async fn bootstrap(&self, req: BootstrapRequest) -> Result<BootstrapResponse, PersonError> {
        // Derive `jkt` from the supplied JWK; reject malformed keys.
        let jkt = jwk_thumbprint(&req.cnf_jwk)
            .map_err(|e| PersonError::BadRequest(format!("cnf_jwk: {e}")))?;

        let agent_id = req
            .agent_id_hint
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("aauth:{}-{}@trogon", short(&jkt), self.clock.now()));

        let iat = self.clock.now();
        let exp = iat + self.agent_jwt_ttl_secs;

        let record = AgentRecord {
            agent_id: agent_id.clone(),
            cnf_jwk: req.cnf_jwk.clone(),
            principal: req.principal.clone(),
            iat,
        };
        self.store
            .put_agent(record)
            .await
            .map_err(|e| PersonError::Store(e.to_string()))?;

        let mut header = Header::new(self.signing_alg);
        header.typ = Some(TYP_AGENT.into());
        header.kid = Some(self.signing_kid.clone());

        let claims = serde_json::json!({
            "iss": self.iss,
            "sub": agent_id,
            "jti": jti(iat, &jkt),
            "iat": iat,
            "exp": exp,
            "dwk": DWK_AGENT,
            "cnf": { "jwk": req.cnf_jwk },
            "ps": self.iss,
        });

        let agent_jwt = encode(&header, &claims, &self.signing_key)
            .map_err(|e| PersonError::Encode(e.to_string()))?;

        Ok(BootstrapResponse {
            agent_jwt,
            agent_id,
            expires_in: self.agent_jwt_ttl_secs,
        })
    }

    pub async fn exchange(&self, req: TokenRequest) -> Result<TokenResponse, PersonError> {
        // Verify the agent token (signature, freshness, typ).
        let agent = self
            .token_verifier
            .verify_agent(&req.agent_jwt)
            .await
            .map_err(|e| PersonError::ResourceTokenInvalid(format!("agent_jwt: {e}")))?;

        // Verify the resource token through the same resolver/JWKS path.
        let resource = self
            .token_verifier
            .verify_resource(&req.resource_jwt, &self.iss)
            .await
            .map_err(|e| PersonError::ResourceTokenInvalid(format!("resource_jwt: {e}")))?;
        let resource_claims = resource.claims;

        if resource_claims.aud != self.iss {
            return Err(PersonError::ResourceTokenInvalid(format!(
                "aud {} did not target this PS ({})",
                resource_claims.aud, self.iss
            )));
        }

        if resource_claims.agent_jkt != agent.jkt {
            return Err(PersonError::ResourceTokenInvalid(
                "agent_jkt mismatch: resource token is bound to a different key".into(),
            ));
        }

        // Consent check.
        let principal = req
            .principal
            .clone()
            .or_else(|| agent.claims.sub.clone().into())
            .unwrap_or_default();
        let decision = self
            .policy
            .decide(&ConsentContext {
                principal: &principal,
                agent_id: &agent.claims.sub,
                resource_iss: &resource_claims.iss,
                requested_scope: &resource_claims.scope,
            })
            .await;

        let (granted_scope, ttl_secs) = match decision {
            ConsentDecision::Allow { granted_scope, ttl_secs } => (granted_scope, ttl_secs),
            ConsentDecision::Interaction { url, code } => {
                return Err(PersonError::RequiresInteraction { url, code });
            }
            ConsentDecision::Deny { reason } => return Err(PersonError::ConsentDenied(reason)),
        };

        let iat = self.clock.now();
        let exp = iat + ttl_secs.min(self.auth_jwt_ttl_secs);

        // Persist consent for auditability.
        self.store
            .put_consent(ConsentRecord {
                principal: principal.clone(),
                agent_id: agent.claims.sub.clone(),
                resource_iss: resource_claims.iss.clone(),
                scope: granted_scope.clone(),
                exp,
            })
            .await
            .map_err(|e| PersonError::Store(e.to_string()))?;

        let mut header = Header::new(self.signing_alg);
        header.typ = Some(TYP_AUTH.into());
        header.kid = Some(self.signing_kid.clone());

        let consent_id = format!("c-{:x}", iat);
        let claims = serde_json::json!({
            "iss": self.iss,
            "sub": agent.claims.sub,
            "aud": resource_claims.iss,
            "jti": jti(iat, &agent.jkt),
            "iat": iat,
            "exp": exp,
            "agent": agent.claims.sub,
            "agent_jkt": agent.jkt,
            "scope": granted_scope,
            "principal": principal,
            "consent_id": consent_id,
            "resource": resource_claims.iss,
        });

        let auth_jwt = encode(&header, &claims, &self.signing_key)
            .map_err(|e| PersonError::Encode(e.to_string()))?;

        Ok(TokenResponse {
            auth_jwt,
            scope: serde_json::from_value::<String>(claims["scope"].clone()).unwrap_or_default(),
            expires_in: exp - iat,
        })
    }

    /// JWKS for this PS (the keys used to sign `aa-agent+jwt` and `aa-auth+jwt`).
    pub fn jwks(&self) -> JwkSet {
        // Callers wire the actual public JWK in via the builder; the core
        // doesn't hold it because EncodingKey is opaque. The HTTP/NATS layer
        // serves the static JwkSet from config.
        JwkSet { keys: vec![] }
    }
}

fn short(s: &str) -> String {
    s.chars().take(8).collect()
}

fn jti(iat: i64, jkt: &str) -> String {
    format!("ps-{:x}-{}", iat, short(jkt))
}

