use tracing::warn;
use trogon_identity_types::ActChainEntry;

use super::cache::{AgentRegistryCache, CachedEntryStatus, DEFAULT_CACHE_TTL};
use super::errors::ChainResolutionError;
use super::mode::ChainResolutionMode;
use super::registry_client::{AgentRegistry, RegistryError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedChain {
    pub entries: Vec<ActChainEntry>,
}

pub struct ChainResolver<R: AgentRegistry> {
    registry: R,
    cache: AgentRegistryCache,
}

impl<R: AgentRegistry> ChainResolver<R> {
    pub fn new(registry: R) -> Self {
        Self {
            registry,
            cache: AgentRegistryCache::new(DEFAULT_CACHE_TTL),
        }
    }

    pub fn with_cache_ttl(registry: R, ttl: std::time::Duration) -> Self {
        Self {
            registry,
            cache: AgentRegistryCache::new(ttl),
        }
    }

    pub async fn resolve(
        &self,
        act_chain: &[ActChainEntry],
        mode: ChainResolutionMode,
    ) -> Result<ResolvedChain, ChainResolutionError> {
        if mode == ChainResolutionMode::Off {
            return Ok(ResolvedChain {
                entries: act_chain.to_vec(),
            });
        }

        for (index, entry) in act_chain.iter().enumerate() {
            let Some(agent_id) = entry.agent_id.as_deref() else {
                continue;
            };
            if agent_id.trim().is_empty() {
                return Err(ChainResolutionError::MalformedEntry { index });
            }

            match self.resolve_agent_id(agent_id, mode).await {
                Ok(()) => {}
                Err(ChainResolutionError::RegistryUnavailable) if mode == ChainResolutionMode::Cache => {
                    warn!(
                        event = "mcp.audit.gateway.chain_resolution_fail_open",
                        entry_index = index,
                        agent_id,
                        "registry unavailable during act_chain resolution; allowing request"
                    );
                }
                Err(err) => return Err(map_error_to_index(err, index, agent_id)),
            }
        }

        Ok(ResolvedChain {
            entries: act_chain.to_vec(),
        })
    }

    async fn resolve_agent_id(
        &self,
        agent_id: &str,
        mode: ChainResolutionMode,
    ) -> Result<(), ChainResolutionError> {
        if mode == ChainResolutionMode::Cache
            && let Some(status) = self.cache.get(agent_id)
        {
            return validate_cached_status(status);
        }

        let status = match self.registry.lookup(agent_id).await {
            Ok(record) => {
                if record.lifecycle_state == "revoked" {
                    CachedEntryStatus::Revoked
                } else {
                    CachedEntryStatus::Active(record)
                }
            }
            Err(RegistryError::NotFound) => CachedEntryStatus::NotFound,
            Err(RegistryError::Revoked) => CachedEntryStatus::Revoked,
            Err(RegistryError::Unavailable(_)) => return Err(ChainResolutionError::RegistryUnavailable),
        };

        if mode == ChainResolutionMode::Cache {
            self.cache.insert(agent_id, status.clone());
        }

        validate_cached_status(status)
    }
}

fn validate_cached_status(status: CachedEntryStatus) -> Result<(), ChainResolutionError> {
    match status {
        CachedEntryStatus::Active(_) => Ok(()),
        CachedEntryStatus::NotFound => Err(ChainResolutionError::Unknown {
            index: 0,
            agent_id: String::new(),
        }),
        CachedEntryStatus::Revoked => Err(ChainResolutionError::Revoked {
            index: 0,
            agent_id: String::new(),
        }),
    }
}

fn map_error_to_index(err: ChainResolutionError, index: usize, agent_id: &str) -> ChainResolutionError {
    match err {
        ChainResolutionError::Unknown { .. } => ChainResolutionError::Unknown {
            index,
            agent_id: agent_id.to_string(),
        },
        ChainResolutionError::Revoked { .. } => ChainResolutionError::Revoked {
            index,
            agent_id: agent_id.to_string(),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use trogon_identity_types::ActChainEntry;

    use super::*;
    use crate::chain_resolver::registry_client::{MockAgentRegistry, RegistryRecord};

    fn entry(sub: &str, agent_id: Option<&str>) -> ActChainEntry {
        ActChainEntry {
            sub: sub.into(),
            agent_id: agent_id.map(str::to_string),
            wkl: None,
            iat: 1,
        }
    }

    fn active_record(agent_id: &str) -> RegistryRecord {
        RegistryRecord {
            agent_id: agent_id.to_string(),
            lifecycle_state: "active".into(),
        }
    }

    fn revoked_record(agent_id: &str) -> RegistryRecord {
        RegistryRecord {
            agent_id: agent_id.to_string(),
            lifecycle_state: "revoked".into(),
        }
    }

    #[tokio::test]
    async fn off_mode_returns_input_unchanged() {
        let registry = MockAgentRegistry::new([]);
        let resolver = ChainResolver::new(registry);
        let chain = vec![entry("user", Some("acme/unknown"))];
        let resolved = resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Off)
            .await
            .expect("off mode resolves");
        assert_eq!(resolved.entries, chain);
    }

    #[tokio::test]
    async fn cache_hit_skips_registry_lookup() {
        let registry = MockAgentRegistry::new([active_record("acme/oncall")]);
        let resolver = ChainResolver::new(registry.clone());
        let chain = vec![entry("user", Some("acme/oncall"))];

        resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Cache)
            .await
            .expect("first resolve");
        assert_eq!(registry.lookup_count(), 1);

        resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Cache)
            .await
            .expect("cached resolve");
        assert_eq!(registry.lookup_count(), 1);
    }

    #[tokio::test]
    async fn cache_miss_populates_cache() {
        let registry = MockAgentRegistry::new([active_record("acme/oncall")]);
        let resolver = ChainResolver::new(registry.clone());
        let chain = vec![entry("user", Some("acme/oncall"))];

        resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Cache)
            .await
            .expect("cache miss resolve");
        assert_eq!(registry.lookup_count(), 1);
        assert_eq!(registry.last_lookup().as_deref(), Some("acme/oncall"));
    }

    #[tokio::test]
    async fn strict_mode_rejects_revoked_entry_at_index() {
        let registry = MockAgentRegistry::new([
            active_record("acme/active"),
            revoked_record("acme/revoked"),
        ]);
        let resolver = ChainResolver::new(registry);
        let chain = vec![
            entry("user", Some("acme/active")),
            entry("agent", Some("acme/revoked")),
        ];

        let err = resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Strict)
            .await
            .expect_err("revoked entry rejected");
        assert_eq!(
            err,
            ChainResolutionError::Revoked {
                index: 1,
                agent_id: "acme/revoked".into(),
            }
        );
    }

    #[tokio::test]
    async fn ttl_expiry_forces_relookup() {
        let registry = MockAgentRegistry::new([active_record("acme/oncall")]);
        let resolver = ChainResolver::with_cache_ttl(registry.clone(), Duration::from_millis(1));
        let chain = vec![entry("user", Some("acme/oncall"))];

        resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Cache)
            .await
            .expect("first resolve");
        tokio::time::sleep(Duration::from_millis(2)).await;
        resolver
            .resolve(chain.as_slice(), ChainResolutionMode::Cache)
            .await
            .expect("second resolve after ttl");
        assert_eq!(registry.lookup_count(), 2);
    }
}
