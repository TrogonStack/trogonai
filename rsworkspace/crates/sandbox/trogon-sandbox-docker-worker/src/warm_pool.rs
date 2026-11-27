//! Pre-started idle Docker containers keyed by snapshot digest.
//!
//! On lease release, `DockerWorkerProvider::cancel` resets warm-pool-eligible
//! containers in place via `docker exec` and returns them here; ineligible
//! leases (real workspace mounts, live checkpoint restores) are torn down and
//! the pool is refilled from a cold start instead.

use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use tokio::time::Instant;

use trogon_sandbox_contract::Digest;

#[derive(Debug, Clone)]
pub struct WarmPoolConfig {
    pub enabled: bool,
    pub max_pool_size_per_digest: usize,
    pub max_total_warm: usize,
    pub max_idle_seconds: u64,
}

impl Default for WarmPoolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_pool_size_per_digest: 3,
            max_total_warm: 16,
            max_idle_seconds: 600,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WarmContainer {
    pub container_id: String,
    pub digest: Digest,
    pub image_ref: String,
    idle_since: Instant,
    last_touch: Instant,
    claim: Option<crate::adapter::native_birth::PoolClaim>,
}

impl WarmContainer {
    pub fn new(container_id: String, digest: Digest, image_ref: String) -> Self {
        let now = Instant::now();
        Self {
            container_id,
            digest,
            image_ref,
            idle_since: now,
            last_touch: now,
            claim: None,
        }
    }

    pub(crate) fn with_claim(mut self, claim: crate::adapter::native_birth::PoolClaim) -> Self {
        self.claim = Some(claim);
        self
    }

    pub(crate) fn claim(&self) -> Option<&crate::adapter::native_birth::PoolClaim> {
        self.claim.as_ref()
    }

    pub fn idle_for(&self) -> Duration {
        self.idle_since.elapsed()
    }

    pub fn touch(&mut self) {
        self.last_touch = Instant::now();
    }
}

pub struct WarmPool {
    config: WarmPoolConfig,
    pools: HashMap<Digest, VecDeque<WarmContainer>>,
}

impl WarmPool {
    pub fn new(config: WarmPoolConfig) -> Self {
        Self {
            config,
            pools: HashMap::new(),
        }
    }

    pub fn config(&self) -> &WarmPoolConfig {
        &self.config
    }

    pub fn take(&mut self, digest: &Digest) -> Option<WarmContainer> {
        let deque = self.pools.get_mut(digest)?;
        deque.pop_front()
    }

    /// Inserts a warm container, evicting LRU entries when limits are exceeded.
    /// Returns container IDs that must be killed by the caller.
    pub fn insert(&mut self, container: WarmContainer) -> Vec<String> {
        let mut evicted = Vec::new();
        let digest = container.digest.clone();

        if let Some(deque) = self.pools.get_mut(&digest) {
            while deque.len() >= self.config.max_pool_size_per_digest {
                if let Some(old) = deque.pop_front() {
                    evicted.push(old.container_id);
                } else {
                    break;
                }
            }
        }

        while self.total_count() >= self.config.max_total_warm {
            match self.evict_lru() {
                Some(id) => evicted.push(id),
                None => break,
            }
        }

        self.pools.entry(digest).or_default().push_back(container);
        evicted
    }

    pub fn count_for_digest(&self, digest: &Digest) -> usize {
        self.pools.get(digest).map(|deque| deque.len()).unwrap_or(0)
    }

    pub fn total_count(&self) -> usize {
        self.pools.values().map(|deque| deque.len()).sum()
    }

    /// Idle containers per digest; a digest whose pool was drained is absent.
    pub fn population(&self) -> HashMap<Digest, u32> {
        self.pools
            .iter()
            .filter(|(_, deque)| !deque.is_empty())
            .map(|(digest, deque)| (digest.clone(), deque.len() as u32))
            .collect()
    }

    pub fn warm_digests(&self) -> Vec<Digest> {
        self.pools.keys().cloned().collect()
    }

    /// Removes idle containers older than `max_idle`, returning IDs to kill.
    pub fn sweep_idle(&mut self, max_idle: Duration) -> Vec<String> {
        let mut killed = Vec::new();
        for deque in self.pools.values_mut() {
            deque.retain(|entry| {
                if entry.idle_for() >= max_idle {
                    killed.push(entry.container_id.clone());
                    false
                } else {
                    true
                }
            });
        }
        self.pools.retain(|_, deque| !deque.is_empty());
        killed
    }

    fn evict_lru(&mut self) -> Option<String> {
        let mut oldest_digest: Option<Digest> = None;
        let mut oldest_index: Option<usize> = None;
        let mut oldest_touch = Instant::now();

        for (digest, deque) in &self.pools {
            for (index, entry) in deque.iter().enumerate() {
                if oldest_index.is_none() || entry.last_touch < oldest_touch {
                    oldest_touch = entry.last_touch;
                    oldest_digest = Some(digest.clone());
                    oldest_index = Some(index);
                }
            }
        }

        let digest = oldest_digest?;
        let index = oldest_index?;
        let deque = self.pools.get_mut(&digest)?;
        let removed = deque.remove(index)?;
        if deque.is_empty() {
            self.pools.remove(&digest);
        }
        Some(removed.container_id)
    }
}

#[cfg(test)]
mod tests;
