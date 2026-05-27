//! Ingress-side `act_chain` registry resolution (complements STS receipt checks).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use moka::future::Cache;
use tracing::warn;
use trogon_identity_types::ActChainEntry;
use trogon_sts::cache::RegistryCache;
use trogon_sts::chain_resolution::ChainResolutionMode;
use trogon_sts::registry::{RegistryLookup, RegistryLookupRequest, RegistryLookupResponse};

use crate::rpc_codes;

const DEFAULT_CHAIN_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct MeshGatewayConfig {
    pub chain_resolution_mode: ChainResolutionMode,
}

impl MeshGatewayConfig {
    pub const ENV_CHAIN_RESOLUTION_MODE: &str = "MCP_GATEWAY_CHAIN_RESOLUTION_MODE";

    pub fn from_env<E: trogon_std::env::ReadEnv>(env: &E) -> Self {
        let raw = env
            .var(Self::ENV_CHAIN_RESOLUTION_MODE)
            .unwrap_or_else(|_| "cache".into());
        Self {
            chain_resolution_mode: parse_chain_resolution_mode(&raw),
        }
    }
}

fn parse_chain_resolution_mode(raw: &str) -> ChainResolutionMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        "off" => ChainResolutionMode::Off,
        "strict" => ChainResolutionMode::Strict,
        _ => ChainResolutionMode::Cache,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WklResolveStatus {
    Known,
    Unresolved,
    Inactive,
}

#[derive(Clone)]
pub struct IngressChainResolver<R: RegistryLookup> {
    registry: RegistryCache<R>,
    wkl_cache: Cache<String, WklResolveStatus>,
    mode: ChainResolutionMode,
}

impl<R: RegistryLookup + Clone> IngressChainResolver<R> {
    pub fn new(registry: RegistryCache<R>, mode: ChainResolutionMode) -> Self {
        let wkl_cache = Cache::builder()
            .time_to_live(DEFAULT_CHAIN_CACHE_TTL)
            .max_capacity(10_000)
            .build();
        Self {
            registry,
            wkl_cache,
            mode,
        }
    }

    /// Returns `Some(deny)` when strict mode blocks the request.
    pub async fn resolve_inbound_chain(
        &self,
        chain: Option<&[ActChainEntry]>,
    ) -> Option<IngressChainDeny> {
        if self.mode == ChainResolutionMode::Off {
            return None;
        }
        let entries = chain.filter(|c| !c.is_empty())?;

        for (index, entry) in entries.iter().enumerate() {
            let status = self.resolve_entry(index, entry).await;
            if status == WklResolveStatus::Known {
                continue;
            }
            let wkl = entry.wkl.as_deref().unwrap_or("");
            let agent_id = entry.agent_id.as_deref();
            warn!(
                event = "mcp.audit.gateway.chain_unresolved",
                entry_index = index,
                wkl,
                agent_id,
                mode = ?self.mode,
                "act_chain entry wkl could not be resolved against registry"
            );
            if self.mode == ChainResolutionMode::Strict {
                return Some(IngressChainDeny {
                    code: rpc_codes::ACT_CHAIN_UNRESOLVED,
                    message: "act_chain_unresolved".into(),
                    entry_index: index,
                });
            }
        }
        None
    }

    async fn resolve_entry(&self, index: usize, entry: &ActChainEntry) -> WklResolveStatus {
        let Some(wkl) = entry.wkl.as_deref().filter(|w| !w.is_empty()) else {
            return WklResolveStatus::Unresolved;
        };
        if is_originator_wkl(wkl) {
            return WklResolveStatus::Known;
        }

        if self.mode == ChainResolutionMode::Cache
            && let Some(status) = self.wkl_cache.get(wkl).await
        {
            return status;
        }

        let status = self.resolve_wkl_via_registry(index, entry, wkl).await;
        if self.mode == ChainResolutionMode::Cache {
            self.wkl_cache.insert(wkl.to_string(), status).await;
        }
        status
    }

    async fn resolve_wkl_via_registry(&self, index: usize, entry: &ActChainEntry, wkl: &str) -> WklResolveStatus {
        let Some(agent_id) = entry.agent_id.as_deref().filter(|a| !a.is_empty()) else {
            return WklResolveStatus::Unresolved;
        };

        let lookup = match self
            .registry
            .lookup_raw(&RegistryLookupRequest {
                agent_id: agent_id.to_string(),
                version: None,
            })
            .await
        {
            Ok(response) => response,
            Err(_) => return WklResolveStatus::Unresolved,
        };

        match lookup {
            RegistryLookupResponse::Ok { record, .. } => {
                if record.lifecycle_state != "active" {
                    warn!(
                        event = "mcp.audit.gateway.chain_unresolved",
                        entry_index = index,
                        wkl,
                        agent_id,
                        lifecycle_state = %record.lifecycle_state,
                        "act_chain entry agent is not active"
                    );
                    return WklResolveStatus::Inactive;
                }
                if record.allowed_workloads.iter().any(|allowed| allowed == wkl) {
                    WklResolveStatus::Known
                } else {
                    WklResolveStatus::Unresolved
                }
            }
            RegistryLookupResponse::NotFound { .. }
            | RegistryLookupResponse::Revoked { .. }
            | RegistryLookupResponse::Error { .. } => WklResolveStatus::Unresolved,
            RegistryLookupResponse::Deprecated { agent_id, .. } => {
                match self.registry.lookup(agent_id.as_str()).await {
                    Ok(record) if record.allowed_workloads.iter().any(|allowed| allowed == wkl) => {
                        WklResolveStatus::Known
                    }
                    _ => WklResolveStatus::Unresolved,
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct IngressChainDeny {
    pub code: i32,
    pub message: String,
    pub entry_index: usize,
}

#[async_trait]
pub trait IngressChainResolve: Send + Sync {
    async fn resolve_inbound_chain(&self, chain: Option<&[ActChainEntry]>) -> Option<IngressChainDeny>;
}

#[async_trait]
impl<R: RegistryLookup + Clone + Send + Sync + 'static> IngressChainResolve for IngressChainResolver<R> {
    async fn resolve_inbound_chain(&self, chain: Option<&[ActChainEntry]>) -> Option<IngressChainDeny> {
        IngressChainResolver::resolve_inbound_chain(self, chain).await
    }
}

pub fn chain_resolver_boxed<R>(registry: RegistryCache<R>, mode: ChainResolutionMode) -> Arc<dyn IngressChainResolve>
where
    R: RegistryLookup + Clone + Send + Sync + 'static,
{
    Arc::new(IngressChainResolver::new(registry, mode))
}

fn is_originator_wkl(wkl: &str) -> bool {
    matches!(wkl, "human" | "batch") || wkl.starts_with("sentinel:")
}
