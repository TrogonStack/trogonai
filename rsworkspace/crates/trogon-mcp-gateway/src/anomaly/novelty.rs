use std::collections::HashMap;
use std::hash::{Hash, Hasher};

const DEFAULT_WINDOW_MS: i64 = 60 * 60 * 1_000;

#[derive(Clone, Debug, Eq)]
struct NoveltyKey {
    tenant_id: String,
    agent_id: String,
    purpose: String,
    target: String,
}

impl PartialEq for NoveltyKey {
    fn eq(&self, other: &Self) -> bool {
        self.tenant_id == other.tenant_id
            && self.agent_id == other.agent_id
            && self.purpose == other.purpose
            && self.target == other.target
    }
}

impl Hash for NoveltyKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.tenant_id.hash(state);
        self.agent_id.hash(state);
        self.purpose.hash(state);
        self.target.hash(state);
    }
}

#[derive(Clone, Debug)]
pub struct NoveltyTracker {
    window_ms: i64,
    last_seen_ms: HashMap<NoveltyKey, i64>,
}

impl Default for NoveltyTracker {
    fn default() -> Self {
        Self::new(DEFAULT_WINDOW_MS)
    }
}

impl NoveltyTracker {
    #[must_use]
    pub fn new(window_ms: i64) -> Self {
        Self {
            window_ms,
            last_seen_ms: HashMap::new(),
        }
    }

    /// Returns `true` when `(tenant_id, agent_id, purpose, target)` is novel for this window.
    pub fn observe(
        &mut self,
        tenant_id: &str,
        agent_id: &str,
        purpose: &str,
        target: &str,
        ts_unix_ms: i64,
    ) -> bool {
        let key = NoveltyKey {
            tenant_id: tenant_id.to_string(),
            agent_id: agent_id.to_string(),
            purpose: purpose.to_string(),
            target: target.to_string(),
        };

        let is_novel = match self.last_seen_ms.get(&key).copied() {
            None => true,
            Some(last_seen_ms) => ts_unix_ms.saturating_sub(last_seen_ms) > self.window_ms,
        };

        self.last_seen_ms.insert(key, ts_unix_ms);
        is_novel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TENANT: &str = "acme";
    const AGENT: &str = "agent/oncall";
    const PURPOSE: &str = "incident_response";
    const TARGET: &str = "urn:trogon:mcp:backend:acme:github";

    #[test]
    fn first_seen_tuple_is_novel() {
        let mut tracker = NoveltyTracker::default();
        assert!(tracker.observe(TENANT, AGENT, PURPOSE, TARGET, 1_000));
    }

    #[test]
    fn second_seen_within_window_is_not_novel() {
        let mut tracker = NoveltyTracker::default();
        assert!(tracker.observe(TENANT, AGENT, PURPOSE, TARGET, 1_000));
        assert!(!tracker.observe(TENANT, AGENT, PURPOSE, TARGET, 1_000 + DEFAULT_WINDOW_MS - 1));
    }

    #[test]
    fn second_seen_after_window_is_novel_again() {
        let mut tracker = NoveltyTracker::default();
        assert!(tracker.observe(TENANT, AGENT, PURPOSE, TARGET, 1_000));
        assert!(tracker.observe(TENANT, AGENT, PURPOSE, TARGET, 1_000 + DEFAULT_WINDOW_MS + 1));
    }
}
