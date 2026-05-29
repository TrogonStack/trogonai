use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::registry_client::RegistryRecord;

pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CachedEntryStatus {
    Active(RegistryRecord),
    NotFound,
    Revoked,
}

struct CacheEntry {
    status: CachedEntryStatus,
    expires_at: Instant,
}

pub struct AgentRegistryCache {
    ttl: Duration,
    inner: Mutex<HashMap<String, CacheEntry>>,
}

impl AgentRegistryCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, agent_id: &str) -> Option<CachedEntryStatus> {
        let mut guard = self.inner.lock().expect("registry cache lock");
        let entry = guard.get(agent_id)?;
        if Instant::now() >= entry.expires_at {
            guard.remove(agent_id);
            return None;
        }
        Some(entry.status.clone())
    }

    pub fn insert(&self, agent_id: &str, status: CachedEntryStatus) {
        let expires_at = Instant::now() + self.ttl;
        self.inner
            .lock()
            .expect("registry cache lock")
            .insert(agent_id.to_string(), CacheEntry { status, expires_at });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(agent_id: &str) -> RegistryRecord {
        RegistryRecord {
            agent_id: agent_id.to_string(),
            lifecycle_state: "active".into(),
        }
    }

    #[test]
    fn expired_entry_is_evicted_on_get() {
        let cache = AgentRegistryCache::new(Duration::from_millis(1));
        cache.insert("acme/agent", CachedEntryStatus::Active(sample_record("acme/agent")));
        std::thread::sleep(Duration::from_millis(2));
        assert!(cache.get("acme/agent").is_none());
    }
}
