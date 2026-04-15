use bytes::Bytes;
use tracing::instrument;

use crate::{
    capability::AgentCapability,
    error::RegistryError,
    store::RegistryStore,
};

/// Live index of every registered agent in the system.
///
/// `Registry<S>` is generic over [`RegistryStore`] so it can be used with the
/// real NATS KV store (`kv::Store`) in production and with
/// `MockRegistryStore` in unit tests.
///
/// ## Typical usage
///
/// ```rust,no_run
/// use trogon_registry::{Registry, provision, AgentCapability};
///
/// async fn example(js: async_nats::jetstream::Context) {
///     let store = provision(&js).await.unwrap();
///     let registry = Registry::new(store);
///
///     let cap = AgentCapability::new("PrActor", ["code_review", "security_analysis"], "actors.pr.>");
///     registry.register(&cap).await.unwrap();
///
///     // Background heartbeat task:
///     // every 15 s: registry.refresh(&cap).await.ok();
///
///     let all = registry.list_all().await.unwrap();
///     let reviewers = registry.discover("code_review").await.unwrap();
/// }
/// ```
#[derive(Clone)]
pub struct Registry<S: RegistryStore> {
    store: S,
}

impl<S: RegistryStore> Registry<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Publish or update a capability record in the registry.
    ///
    /// This is also the heartbeat operation — call it every
    /// [`crate::provision::HEARTBEAT_INTERVAL`] to keep the entry alive.
    /// The `current_load` field can be updated before each heartbeat call to
    /// give the Router up-to-date load information.
    #[instrument(skip(self, capability), fields(agent_type = %capability.agent_type), err)]
    pub async fn register(&self, capability: &AgentCapability) -> Result<(), RegistryError> {
        let value =
            serde_json::to_vec(capability).map_err(RegistryError::Serialization)?;
        self.store
            .put(&capability.agent_type, Bytes::from(value))
            .await
    }

    /// Re-publish a capability record to reset its TTL — the heartbeat.
    ///
    /// Semantically identical to [`register`][Self::register] but named
    /// separately to make call-site intent clear.
    pub async fn refresh(&self, capability: &AgentCapability) -> Result<(), RegistryError> {
        self.register(capability).await
    }

    /// Remove an agent's capability record from the registry immediately.
    ///
    /// Agents that crash do not need to call this — their entry will expire
    /// on its own after [`crate::provision::ENTRY_TTL`]. Call `unregister`
    /// only on clean shutdown.
    #[instrument(skip(self), fields(agent_type), err)]
    pub async fn unregister(&self, agent_type: &str) -> Result<(), RegistryError> {
        self.store.delete(agent_type).await
    }

    /// Return all agents that advertise `capability` (case-insensitive match).
    ///
    /// The Router calls this to build its context for the LLM routing prompt.
    pub async fn discover(&self, capability: &str) -> Result<Vec<AgentCapability>, RegistryError> {
        let all = self.list_all().await?;
        Ok(all
            .into_iter()
            .filter(|c| c.has_capability(capability))
            .collect())
    }

    /// Return every currently-registered agent.
    ///
    /// This is a snapshot — agents may register or expire between calls.
    #[instrument(skip(self), err)]
    pub async fn list_all(&self) -> Result<Vec<AgentCapability>, RegistryError> {
        let keys = self.store.keys().await?;
        let mut capabilities = Vec::with_capacity(keys.len());

        for key in keys {
            match self.store.get(&key).await? {
                Some(bytes) => match serde_json::from_slice::<AgentCapability>(&bytes) {
                    Ok(cap) => capabilities.push(cap),
                    Err(e) => {
                        tracing::warn!(
                            key,
                            error = %e,
                            "failed to deserialize capability record — skipping"
                        );
                    }
                },
                None => {
                    // Key expired between `keys()` and `get()` — normal race, ignore.
                }
            }
        }

        Ok(capabilities)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::mock::MockRegistryStore;

    // ── TOCTOU store ──────────────────────────────────────────────────────────
    //
    // A store whose `keys()` lists one entry but whose `get()` always returns
    // `None`, simulating the real race where a KV entry's TTL expires between
    // the `keys()` call and the subsequent `get()` call in `list_all()`.

    #[derive(Clone)]
    struct VanishingStore;

    impl crate::store::RegistryStore for VanishingStore {
        async fn put(&self, _key: &str, _value: bytes::Bytes) -> Result<(), RegistryError> {
            Ok(())
        }
        async fn get(&self, _key: &str) -> Result<Option<bytes::Bytes>, RegistryError> {
            Ok(None) // simulate TTL expiry — key listed but then gone
        }
        async fn delete(&self, _key: &str) -> Result<(), RegistryError> {
            Ok(())
        }
        async fn keys(&self) -> Result<Vec<String>, RegistryError> {
            Ok(vec!["PhantomAgent".to_string()])
        }
    }

    fn pr_actor() -> AgentCapability {
        AgentCapability::new(
            "PrActor",
            ["code_review", "security_analysis"],
            "actors.pr.>",
        )
    }

    fn incident_actor() -> AgentCapability {
        AgentCapability::new(
            "IncidentActor",
            ["triage", "timeline", "escalation"],
            "actors.incident.>",
        )
    }

    fn registry() -> Registry<MockRegistryStore> {
        Registry::new(MockRegistryStore::new())
    }

    #[tokio::test]
    async fn register_then_list_all() {
        let r = registry();
        r.register(&pr_actor()).await.unwrap();
        r.register(&incident_actor()).await.unwrap();

        let all = r.list_all().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn discover_by_capability_case_insensitive() {
        let r = registry();
        r.register(&pr_actor()).await.unwrap();
        r.register(&incident_actor()).await.unwrap();

        let found = r.discover("code_review").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].agent_type, "PrActor");

        let found_upper = r.discover("CODE_REVIEW").await.unwrap();
        assert_eq!(found_upper.len(), 1);
    }

    #[tokio::test]
    async fn discover_no_match_returns_empty() {
        let r = registry();
        r.register(&pr_actor()).await.unwrap();

        let found = r.discover("unknown_capability").await.unwrap();
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn unregister_removes_entry() {
        let r = registry();
        r.register(&pr_actor()).await.unwrap();
        r.register(&incident_actor()).await.unwrap();

        r.unregister("PrActor").await.unwrap();

        let all = r.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].agent_type, "IncidentActor");
    }

    #[tokio::test]
    async fn refresh_is_idempotent() {
        let r = registry();
        let mut cap = pr_actor();
        r.register(&cap).await.unwrap();

        cap.current_load = 5;
        r.refresh(&cap).await.unwrap();

        let all = r.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].current_load, 5);
    }

    #[tokio::test]
    async fn list_all_on_empty_registry() {
        let r = registry();
        let all = r.list_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn register_overwrites_previous_entry() {
        let r = registry();
        let cap = pr_actor();
        r.register(&cap).await.unwrap();

        let mut updated = cap.clone();
        updated.current_load = 10;
        updated.capabilities.push("dependency_check".to_string());
        r.register(&updated).await.unwrap();

        let all = r.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].current_load, 10);
        assert!(all[0].has_capability("dependency_check"));
    }

    #[tokio::test]
    async fn has_capability_is_case_insensitive() {
        let cap = pr_actor();
        assert!(cap.has_capability("code_review"));
        assert!(cap.has_capability("CODE_REVIEW"));
        assert!(cap.has_capability("Code_Review"));
        assert!(!cap.has_capability("triage"));
    }

    #[tokio::test]
    async fn mock_store_snapshot_reflects_registered_agents() {
        let store = MockRegistryStore::new();
        let r = Registry::new(store.clone());
        r.register(&pr_actor()).await.unwrap();
        r.register(&incident_actor()).await.unwrap();

        let snap = store.snapshot();
        assert_eq!(snap.len(), 2);
        assert!(snap.contains_key("PrActor"));
        assert!(snap.contains_key("IncidentActor"));
    }

    /// `list_all` must silently skip a key that vanishes between `keys()` and
    /// `get()` — the normal NATS KV TTL race. The returned list must be empty
    /// (not an error) even though `keys()` reported one entry.
    #[tokio::test]
    async fn list_all_ignores_key_that_expires_between_keys_and_get() {
        let r = Registry::new(VanishingStore);
        let all = r.list_all().await.unwrap();
        assert!(
            all.is_empty(),
            "key that vanishes between keys() and get() should be silently skipped"
        );
    }

    #[tokio::test]
    async fn list_all_skips_corrupted_entries() {
        // Cover the Err(e) deserialization branch in list_all: put raw bytes
        // that are not valid JSON into the store, verify the good entries
        // are still returned and the bad one is silently skipped.
        use crate::store::RegistryStore as _;
        let store = MockRegistryStore::new();
        let r = Registry::new(store.clone());
        r.register(&pr_actor()).await.unwrap();
        store.put("Corrupted", bytes::Bytes::from_static(b"not-valid-json")).await.unwrap();

        let all = r.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].agent_type, "PrActor");
    }
}
