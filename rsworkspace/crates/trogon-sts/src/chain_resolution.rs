use std::time::Duration;

use moka::future::Cache;
use tracing::warn;
use trogon_identity_types::ActChainEntry;

use crate::cache::RegistryCache;
use crate::error::StsError;
use crate::registry::{RegistryLookup, RegistryLookupRequest, RegistryLookupResponse};

const DEFAULT_CHAIN_CACHE_TTL: Duration = Duration::from_secs(60);
const CHAIN_WALK_BUDGET: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainResolutionMode {
    Off,
    Cache,
    Strict,
}

impl ChainResolutionMode {
    pub fn from_env() -> Self {
        match std::env::var("STS_CHAIN_RESOLUTION_MODE")
            .unwrap_or_else(|_| "cache".into())
            .to_ascii_lowercase()
            .as_str()
        {
            "off" => Self::Off,
            "strict" => Self::Strict,
            _ => Self::Cache,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainEntryStatus {
    Active,
    NotFound,
    Revoked,
}

#[derive(Clone)]
pub struct ChainResolver<R: RegistryLookup> {
    registry: RegistryCache<R>,
    cache: Cache<String, ChainEntryStatus>,
    mode: ChainResolutionMode,
}

impl<R: RegistryLookup + Clone> ChainResolver<R> {
    pub fn new(registry: RegistryCache<R>, mode: ChainResolutionMode) -> Self {
        let cache = Cache::builder()
            .time_to_live(DEFAULT_CHAIN_CACHE_TTL)
            .max_capacity(10_000)
            .build();
        Self {
            registry,
            cache,
            mode,
        }
    }

    pub async fn verify_inbound_chain(&self, chain: &[ActChainEntry]) -> Result<(), StsError> {
        if self.mode == ChainResolutionMode::Off {
            return Ok(());
        }

        let started = std::time::Instant::now();
        for (index, entry) in chain.iter().enumerate() {
            let Some(agent_id) = entry.agent_id.as_deref() else {
                continue;
            };
            let status = self.resolve_entry(agent_id).await?;
            if matches!(status, ChainEntryStatus::NotFound | ChainEntryStatus::Revoked) {
                return Err(StsError::ActChainEntryRevoked {
                    index,
                    agent_id: agent_id.to_string(),
                });
            }
        }

        let elapsed = started.elapsed();
        if elapsed > CHAIN_WALK_BUDGET {
            warn!(
                elapsed_ms = elapsed.as_millis(),
                chain_len = chain.len(),
                "act_chain registry walk exceeded latency budget"
            );
        }
        Ok(())
    }

    async fn resolve_entry(&self, agent_id: &str) -> Result<ChainEntryStatus, StsError> {
        if self.mode == ChainResolutionMode::Cache
            && let Some(status) = self.cache.get(agent_id).await
        {
            return Ok(status);
        }

        let status = match self
            .registry
            .lookup_raw(&RegistryLookupRequest {
                agent_id: agent_id.to_string(),
                version: None,
            })
            .await?
        {
            RegistryLookupResponse::Ok { record, .. } => {
                if record.lifecycle_state == "revoked" {
                    ChainEntryStatus::Revoked
                } else {
                    ChainEntryStatus::Active
                }
            }
            RegistryLookupResponse::NotFound { .. } => ChainEntryStatus::NotFound,
            RegistryLookupResponse::Revoked { .. } => ChainEntryStatus::Revoked,
            RegistryLookupResponse::Deprecated { .. } => ChainEntryStatus::Active,
            RegistryLookupResponse::Error { reason } => {
                return Err(StsError::RegistryUnavailable(reason));
            }
        };

        if self.mode == ChainResolutionMode::Cache {
            self.cache.insert(agent_id.to_string(), status).await;
        }
        Ok(status)
    }
}
