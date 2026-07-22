use std::hash::Hash;
use std::time::{Duration, Instant};

use moka::future::Cache;

use super::config::ZedTokenTtl;
use crate::agent_id::A2aAgentId;
use crate::catalog::import_gate::ImportedAccountName;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZedTokenSnapshot {
    pub token: String,
    pub observed_at: Instant,
}

impl ZedTokenSnapshot {
    pub fn is_fresh(&self, ttl: Duration) -> bool {
        self.observed_at.elapsed() <= ttl
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImportGateCacheKey {
    imported_from: ImportedAccountName,
    agent_id: A2aAgentId,
}

impl ImportGateCacheKey {
    pub fn new(imported_from: &ImportedAccountName, agent_id: &A2aAgentId) -> Self {
        Self {
            imported_from: imported_from.clone(),
            agent_id: agent_id.clone(),
        }
    }
}

#[derive(Clone)]
pub struct ZedTokenCache {
    inner: Cache<ImportGateCacheKey, ZedTokenSnapshot>,
    ttl: ZedTokenTtl,
}

impl ZedTokenCache {
    pub fn new(ttl: ZedTokenTtl) -> Self {
        Self {
            inner: Cache::builder().max_capacity(1024).build(),
            ttl,
        }
    }

    pub fn ttl(&self) -> ZedTokenTtl {
        self.ttl
    }

    pub async fn get(&self, key: &ImportGateCacheKey) -> Option<ZedTokenSnapshot> {
        let snapshot = self.inner.get(key).await?;
        if snapshot.is_fresh(self.ttl.as_duration()) {
            Some(snapshot)
        } else {
            self.inner.invalidate(key).await;
            None
        }
    }

    pub async fn insert(&self, key: ImportGateCacheKey, token: String) {
        self.inner
            .insert(
                key,
                ZedTokenSnapshot {
                    token,
                    observed_at: Instant::now(),
                },
            )
            .await;
    }
}
