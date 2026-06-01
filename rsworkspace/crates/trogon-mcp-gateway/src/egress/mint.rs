use std::sync::Arc;

use async_nats::HeaderMap;
use tracing::{info, warn};
use trogon_sts_client::{MintedMeshToken, StsClient, StsClientError, build_exchange_request};

use crate::agent_identity::AgentIdentityMode;
use crate::egress::audience::{
    backend_target_aud, client_target_aud, peek_token_audience, publish_shadow_aud_mismatch_metric,
    record_shadow_aud_mismatch,
};
use crate::egress::cache::{CacheKeyParts, MCP_SESSION_HEADER, MeshEgressCache, scope_fingerprint, tool_scope};
use crate::egress::config::EgressMintConfig;
use crate::jwt::VerifiedJwtClaims;
use crate::rpc_codes;

pub const A2A_CALLER_JWT_HEADER: &str = "A2a-Caller-Jwt";

#[derive(Clone, Debug)]
pub enum EgressTarget {
    Backend { server_id: String },
    Client { client_id: String },
}

#[derive(Debug, Clone)]
pub struct EgressMintError {
    pub code: i32,
    pub message: String,
}

impl From<StsClientError> for EgressMintError {
    fn from(value: StsClientError) -> Self {
        Self {
            code: value.gateway_rpc_code(),
            message: value.gateway_rpc_message(),
        }
    }
}

#[derive(Clone)]
pub struct EgressMinter {
    sts: StsClient,
    cache: MeshEgressCache,
    config: EgressMintConfig,
    metrics: Option<Arc<async_nats::Client>>,
}

impl EgressMinter {
    pub fn new(sts: StsClient, config: EgressMintConfig, metrics: Option<Arc<async_nats::Client>>) -> Self {
        let cache = MeshEgressCache::new(config.clone());
        Self {
            sts,
            cache,
            config,
            metrics,
        }
    }

    pub fn from_parts(
        sts: StsClient,
        config: EgressMintConfig,
        metrics: Option<Arc<async_nats::Client>>,
    ) -> Arc<Self> {
        Arc::new(Self::new(sts, config, metrics))
    }

    pub fn config(&self) -> &EgressMintConfig {
        &self.config
    }

    pub fn target_audience(&self, tenant: &str, target: &EgressTarget) -> String {
        match target {
            EgressTarget::Backend { server_id } => backend_target_aud(tenant, server_id),
            EgressTarget::Client { client_id } => client_target_aud(tenant, client_id),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn mint(
        &self,
        mode: AgentIdentityMode,
        subject_token: Option<&str>,
        tenant: &str,
        caller_sub: &str,
        session_id: &str,
        scope: Option<&str>,
        purpose: Option<&str>,
        target: EgressTarget,
    ) -> Result<Option<MintedMeshToken>, EgressMintError> {
        if mode == AgentIdentityMode::Off {
            return Ok(None);
        }

        let Some(subject_token) = subject_token.filter(|s| !s.is_empty()) else {
            if mode == AgentIdentityMode::Enforce {
                return Err(EgressMintError {
                    code: rpc_codes::AUTH_REQUIRED,
                    message: "authentication required".into(),
                });
            }
            warn!(
                event = "mesh_egress_skip_no_subject",
                "mesh egress skipped: no subject token in shadow mode"
            );
            return Ok(None);
        };

        let target_aud = self.target_audience(tenant, &target);
        if mode == AgentIdentityMode::Shadow
            && let Some(presented_aud) = peek_token_audience(subject_token)
            && record_shadow_aud_mismatch(target_aud.as_str(), presented_aud.as_str(), tenant, caller_sub)
            && let Some(client) = self.metrics.as_ref()
        {
            publish_shadow_aud_mismatch_metric(
                client.as_ref(),
                self.config.prefix.as_str(),
                target_aud.as_str(),
                presented_aud.as_str(),
                tenant,
                caller_sub,
            )
            .await;
        }
        let key = CacheKeyParts {
            tenant: tenant.to_string(),
            caller_sub: caller_sub.to_string(),
            target_aud: target_aud.clone(),
            session_id: session_id.to_string(),
            scope_fingerprint: scope_fingerprint(scope),
        };

        if let Some(cached) = self.cache.lookup(&key).await {
            if self.cache.needs_proactive_refresh(&key, &cached) {
                let minter = self.clone();
                let refresh_key = key.clone();
                let refresh_target = target.clone();
                let subject = subject_token.to_string();
                let scope_owned = scope.map(str::to_string);
                let purpose_owned = purpose.map(str::to_string);
                tokio::spawn(async move {
                    if let Err(e) = minter
                        .exchange_and_store(
                            &refresh_key,
                            &subject,
                            &refresh_target,
                            scope_owned.as_deref(),
                            purpose_owned.as_deref(),
                        )
                        .await
                    {
                        warn!(event = "mesh_cache_refresh_failed", error = %e.message, "background mesh cache refresh failed");
                    } else {
                        info!(event = "mesh_cache_refresh", cache_key = %refresh_key.cache_key(), "mesh egress cache refreshed");
                    }
                });
            }
            return Ok(Some(cached));
        }

        self.exchange_and_store(&key, subject_token, &target, scope, purpose)
            .await
            .map(Some)
    }

    async fn exchange_and_store(
        &self,
        key: &CacheKeyParts,
        subject_token: &str,
        _target: &EgressTarget,
        scope: Option<&str>,
        purpose: Option<&str>,
    ) -> Result<MintedMeshToken, EgressMintError> {
        let audience = key.target_aud.clone();
        let request = build_exchange_request(
            subject_token,
            self.config.actor_token.as_str(),
            audience.as_str(),
            scope.unwrap_or(""),
            purpose.unwrap_or(""),
        );
        let token = self.sts.exchange(request).await?;
        self.cache.store(key, token.clone()).await;
        Ok(token)
    }
}

pub fn session_id_from_headers(headers: Option<&HeaderMap>, claims: &VerifiedJwtClaims) -> String {
    if let Some(id) = claims.session_id.as_deref().filter(|s| !s.is_empty()) {
        return id.to_string();
    }
    let Some(hm) = headers else {
        return "*".to_string();
    };
    hm.get_last(MCP_SESSION_HEADER)
        .or_else(|| hm.get(MCP_SESSION_HEADER))
        .map(|v| v.as_str().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "*".to_string())
}

pub fn scope_for_tools_call(server_id: &str, tool_name: Option<&str>) -> Option<String> {
    tool_name.map(|name| tool_scope(server_id, name))
}

pub fn apply_mesh_egress_headers(
    headers: &mut HeaderMap,
    mesh_token: &MintedMeshToken,
    mode: AgentIdentityMode,
    bearer_header_normalized: &str,
    shadow_propagate_legacy: bool,
) {
    strip_header(headers, bearer_header_normalized);
    strip_header(headers, A2A_CALLER_JWT_HEADER);
    let authorization = format!("Bearer {}", mesh_token.access_token);
    headers.insert(bearer_header_normalized, authorization.as_str());

    if mode == AgentIdentityMode::Shadow && shadow_propagate_legacy {
        info!(
            event = "mesh_egress_shadow_legacy",
            iss = %mesh_token.iss,
            "shadow mode minted mesh token while legacy headers remain on egress"
        );
    }
}

pub fn strip_inbound_credentials(headers: &mut HeaderMap, bearer_header_normalized: &str) {
    strip_header(headers, bearer_header_normalized);
    strip_header(headers, A2A_CALLER_JWT_HEADER);
}

fn strip_header(headers: &mut HeaderMap, name: &str) {
    let mut rebuilt = HeaderMap::new();
    for (header_name, vals) in headers.iter() {
        if AsRef::<str>::as_ref(&header_name).eq_ignore_ascii_case(name) {
            continue;
        }
        for v in vals {
            rebuilt.append(header_name.clone(), v.clone());
        }
    }
    *headers = rebuilt;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_prefers_claim() {
        let claims = VerifiedJwtClaims {
            session_id: Some("sess-claim".into()),
            ..VerifiedJwtClaims::default()
        };
        assert_eq!(session_id_from_headers(None, &claims), "sess-claim");
    }
}
