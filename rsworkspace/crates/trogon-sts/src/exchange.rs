use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use trogon_identity_types::{ActChainEntry, MAX_ACT_CHAIN_DEPTH};

use crate::audit::{AuditPublisher, StsAuditEmit, emit_audit};
use crate::cache::{JwksCache, RegistryCache, TrustBundleCache};
use crate::chain_resolution::{ChainResolutionMode, ChainResolver};
use crate::error::StsError;
use crate::limits::RateLimiter;
use crate::registry::RegistryLookup;
use crate::signer::DynSigner;
use crate::spicedb::{NoOpSpiceDb, SpiceDbCheck};
use crate::token_verify::verify_subject_token;
use crate::trust::verify_actor_token;
use crate::types::{MintedClaimsSummary, StsExchangeRequest, StsExchangeResponse, StsTokenErrorResponse};
use crate::{DEFAULT_MESH_TOKEN_TTL_SECS, MAX_MESH_TOKEN_TTL_SECS, MIN_MESH_TOKEN_TTL_SECS};

pub type ExchangeRequest = StsExchangeRequest;
pub type ExchangeSuccess = StsExchangeResponse;

pub struct ExchangeService<R, A, S = NoOpSpiceDb>
where
    R: RegistryLookup,
    S: SpiceDbCheck,
{
    mesh_issuer: String,
    bootstrap_issuer: String,
    jwks: JwksCache,
    trust_bundle: TrustBundleCache,
    registry: RegistryCache<R>,
    chain_resolver: ChainResolver<R>,
    spicedb: S,
    signer: DynSigner,
    limits: RateLimiter,
    audit: Arc<A>,
}

impl<R, A, S> ExchangeService<R, A, S>
where
    R: RegistryLookup + Clone,
    A: AuditPublisher,
    S: SpiceDbCheck,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mesh_issuer: String,
        bootstrap_issuer: String,
        jwks: JwksCache,
        trust_bundle: TrustBundleCache,
        registry: RegistryCache<R>,
        signer: DynSigner,
        audit: Arc<A>,
        spicedb: S,
        chain_resolution_mode: ChainResolutionMode,
    ) -> Self {
        let chain_resolver = ChainResolver::new(registry.clone(), chain_resolution_mode);
        Self {
            mesh_issuer,
            bootstrap_issuer,
            jwks,
            trust_bundle,
            registry,
            chain_resolver,
            spicedb,
            signer,
            limits: RateLimiter::new(),
            audit,
        }
    }

    pub async fn handle(
        &self,
        request: StsExchangeRequest,
        source_ip: Option<String>,
    ) -> Result<StsExchangeResponse, StsTokenErrorResponse> {
        let started = Instant::now();
        match self.exchange_inner(&request).await {
            Ok((response, summary, wkl, agent_id)) => {
                emit_audit(
                    self.audit.as_ref(),
                    StsAuditEmit {
                        outcome: "success",
                        reason: "exchange_ok".into(),
                        latency: started.elapsed(),
                        request: &request,
                        wkl: Some(wkl),
                        agent_id,
                        minted: Some(summary),
                        source_ip,
                        offending_index: None,
                        offending_agent_id: None,
                    },
                )
                .await;
                Ok(response)
            }
            Err(err) => {
                let (offending_index, offending_agent_id) = match &err {
                    StsError::ActChainEntryRevoked { index, agent_id } => {
                        (Some(*index), Some(agent_id.clone()))
                    }
                    _ => (None, None),
                };
                let outcome = err.audit_outcome();
                emit_audit(
                    self.audit.as_ref(),
                    StsAuditEmit {
                        outcome,
                        reason: err.audit_reason(),
                        latency: started.elapsed(),
                        request: &request,
                        wkl: None,
                        agent_id: None,
                        minted: None,
                        source_ip,
                        offending_index,
                        offending_agent_id,
                    },
                )
                .await;
                Err(StsTokenErrorResponse::from_sts_error(&err))
            }
        }
    }

    async fn exchange_inner(
        &self,
        request: &StsExchangeRequest,
    ) -> Result<(StsExchangeResponse, MintedClaimsSummary, String, Option<String>), StsError> {
        validate_wire_request(request)?;

        let iss_hint = peek_token_iss(&request.subject_token)?;
        let (iss, jwks) = if iss_hint == self.mesh_issuer {
            (self.mesh_issuer.clone(), self.jwks.mesh_jwks().await)
        } else {
            (self.bootstrap_issuer.clone(), self.jwks.bootstrap_jwks().await)
        };

        let subject = verify_subject_token(&request.subject_token, &iss, &jwks)?;

        self.spicedb.authorize_exchange().await?;

        self.chain_resolver
            .verify_inbound_chain(&subject.act_chain)
            .await?;

        let trust_pem = self.trust_bundle.pem().await;
        let actor_wkl = verify_actor_token(&request.actor_token, &trust_pem)?;

        self.limits.check(&actor_wkl, subject.agent_id.as_deref())?;

        let registry_record = if let Some(agent_id) = subject.agent_id.as_deref() {
            Some(self.registry.lookup(agent_id).await?)
        } else {
            None
        };

        if let Some(record) = &registry_record {
            assert_wkl_allowed(&actor_wkl, record)?;
            assert_audience_allowed(&request.audience, record)?;
            assert_purpose_allowed(&request.purpose, record)?;
        }

        let mut act_chain = subject.act_chain.clone();
        if act_chain.len() >= MAX_ACT_CHAIN_DEPTH {
            return Err(StsError::ActChainDepthExceeded);
        }

        let new_entry = ActChainEntry {
            sub: subject.sub.clone(),
            agent_id: subject.agent_id.clone(),
            wkl: Some(actor_wkl.clone()),
            iat: now_unix(),
        };
        if act_chain
            .iter()
            .any(|e| e.agent_id == new_entry.agent_id && e.wkl == new_entry.wkl)
        {
            return Err(StsError::ActChainLoopDetected);
        }
        act_chain.push(new_entry);

        let ttl = mesh_token_ttl(registry_record.as_ref());
        let iat = now_unix();
        let exp = iat + i64::try_from(ttl).unwrap_or(DEFAULT_MESH_TOKEN_TTL_SECS as i64);

        let scope = narrow_scope(&request.scope, registry_record.as_ref());
        let mut claims: HashMap<String, Value> = HashMap::new();
        claims.insert("sub".into(), json!(subject.sub));
        claims.insert("iss".into(), json!(self.mesh_issuer));
        claims.insert("aud".into(), json!(request.audience));
        claims.insert("iat".into(), json!(iat));
        claims.insert("exp".into(), json!(exp));
        claims.insert("scope".into(), json!(scope.clone()));
        claims.insert("purpose".into(), json!(request.purpose));
        claims.insert("wkl".into(), json!(actor_wkl));
        claims.insert("act_chain".into(), json!(act_chain));
        if let Some(agent_id) = &subject.agent_id {
            claims.insert("agent_id".into(), json!(agent_id));
        }
        if let Some(version) = &subject.agent_version {
            claims.insert("agent_version".into(), json!(version));
        } else if let Some(record) = &registry_record {
            claims.insert("agent_version".into(), json!(record.agent_version.clone()));
        }
        if let Some(tenant) = &subject.tenant {
            claims.insert("tenant".into(), json!(tenant));
        }

        let access_token = self.signer.sign(&claims).await?;
        let summary = MintedClaimsSummary {
            sub: subject.sub.clone(),
            iss: self.mesh_issuer.clone(),
            aud: request.audience.clone(),
            exp,
            iat,
            agent_id: subject.agent_id.clone(),
            wkl: Some(actor_wkl.clone()),
            purpose: Some(request.purpose.clone()),
            scope,
            act_chain_depth: act_chain.len(),
        };

        Ok((
            StsExchangeResponse {
                access_token,
                issued_token_type: request.requested_token_type.clone(),
                token_type_bearer: "Bearer".into(),
                expires_in: ttl,
            },
            summary,
            actor_wkl,
            subject.agent_id,
        ))
    }
}

fn validate_wire_request(request: &StsExchangeRequest) -> Result<(), StsError> {
    if request.subject_token.trim().is_empty() {
        return Err(StsError::InvalidRequest("subject_token required".into()));
    }
    if request.subject_token_type != "urn:ietf:params:oauth:token-type:jwt" {
        return Err(StsError::InvalidRequest("unsupported subject_token_type".into()));
    }
    if request.requested_token_type != "urn:ietf:params:oauth:token-type:jwt" {
        return Err(StsError::InvalidRequest("unsupported requested_token_type".into()));
    }
    if request.audience.trim().is_empty() {
        return Err(StsError::InvalidRequest("audience required".into()));
    }
    Ok(())
}

fn peek_token_iss(token: &str) -> Result<String, StsError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return Err(StsError::InvalidGrant("malformed jwt".into()));
    }
    let payload = base64_decode_url(parts[1])?;
    let value: Value =
        serde_json::from_slice(&payload).map_err(|e| StsError::InvalidGrant(format!("jwt payload json: {e}")))?;
    value
        .get("iss")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| StsError::InvalidGrant("jwt missing iss".into()))
}

fn base64_decode_url(input: &str) -> Result<Vec<u8>, StsError> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
        .map_err(|e| StsError::InvalidGrant(format!("jwt base64: {e}")))
}

fn assert_wkl_allowed(wkl: &str, record: &crate::registry::AgentRegistryRecord) -> Result<(), StsError> {
    if record.allowed_workloads.iter().any(|allowed| allowed == wkl) {
        Ok(())
    } else {
        Err(StsError::AccessDenied(format!(
            "wkl {wkl} not in allowed_workloads for {}",
            record.agent_id
        )))
    }
}

fn assert_audience_allowed(audience: &str, record: &crate::registry::AgentRegistryRecord) -> Result<(), StsError> {
    if record.allowed_audiences.iter().any(|a| a == audience) {
        Ok(())
    } else {
        Err(StsError::InvalidTarget(format!(
            "audience {audience} not permitted for {}",
            record.agent_id
        )))
    }
}

fn assert_purpose_allowed(purpose: &str, record: &crate::registry::AgentRegistryRecord) -> Result<(), StsError> {
    if record.allowed_purposes.is_empty() {
        return Ok(());
    }
    if record.allowed_purposes.iter().any(|p| p == purpose) {
        Ok(())
    } else {
        Err(StsError::AccessDenied(format!(
            "purpose {purpose} not permitted for {}",
            record.agent_id
        )))
    }
}

fn mesh_token_ttl(record: Option<&crate::registry::AgentRegistryRecord>) -> u64 {
    let ttl = record
        .and_then(|r| r.mesh_token_ttl_s)
        .unwrap_or(DEFAULT_MESH_TOKEN_TTL_SECS);
    ttl.clamp(MIN_MESH_TOKEN_TTL_SECS, MAX_MESH_TOKEN_TTL_SECS)
}

fn narrow_scope(requested: &str, record: Option<&crate::registry::AgentRegistryRecord>) -> String {
    let requested_tools: Vec<&str> = requested.split_whitespace().collect();
    if let Some(record) = record {
        requested_tools
            .into_iter()
            .filter(|tool| {
                record.allowed_tools.iter().any(|allowed| allowed == *tool)
                    || record.allowed_tools.contains(&"*".to_string())
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        requested.to_string()
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn default_bootstrap_issuer() -> String {
    "https://id.trogon.ai/callout".to_string()
}

#[cfg(test)]
pub mod test_helpers {
    use jsonwebtoken::jwk::JwkSet;

    pub fn empty_jwks() -> JwkSet {
        JwkSet { keys: vec![] }
    }
}
