use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};
use tracing::warn;

const CONSOLE_AGENTS_BUCKET: &str = "CONSOLE_AGENTS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait AgentLoading: Send + Sync + 'static {
    fn get_skill_ids<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Vec<String>>;
}

// ── Real implementation ───────────────────────────────────────────────────────

/// Reads the agent definition from the console KV store and returns its skill_ids.
/// Called on every chat message turn so updates to the agent definition propagate
/// without an agent restart.
#[derive(Clone)]
pub struct AgentLoader {
    agents_kv: kv::Store,
}

impl AgentLoader {
    pub async fn open(js: &jetstream::Context) -> Result<Self, String> {
        let agents_kv = js
            .create_or_update_key_value(kv::Config {
                bucket: CONSOLE_AGENTS_BUCKET.to_string(),
                history: 1,
                ..Default::default()
            })
            .await
            .map_err(|e| e.to_string())?;

        Ok(Self { agents_kv })
    }

    async fn get_skill_ids_impl(&self, agent_id: &str) -> Vec<String> {
        match self.agents_kv.get(agent_id).await {
            Ok(Some(bytes)) => {
                let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    warn!(agent_id, "CONSOLE_AGENTS entry is not valid JSON");
                    return vec![];
                };
                val["skill_ids"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default()
            }
            Ok(None) => {
                warn!(agent_id, "agent not found in CONSOLE_AGENTS — no skills will be injected");
                vec![]
            }
            Err(e) => {
                warn!(agent_id, error = %e, "Failed to read CONSOLE_AGENTS");
                vec![]
            }
        }
    }
}

impl AgentLoading for AgentLoader {
    fn get_skill_ids<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, Vec<String>> {
        Box::pin(self.get_skill_ids_impl(agent_id))
    }
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(feature = "test-helpers")]
pub mod mock {
    use super::AgentLoading;
    use std::collections::HashMap;

    pub struct MockAgentLoader {
        skill_ids: HashMap<String, Vec<String>>,
    }

    impl MockAgentLoader {
        pub fn new() -> Self {
            Self { skill_ids: HashMap::new() }
        }

        pub fn insert(&mut self, agent_id: &str, skill_ids: Vec<String>) {
            self.skill_ids.insert(agent_id.to_string(), skill_ids);
        }
    }

    impl AgentLoading for MockAgentLoader {
        fn get_skill_ids<'a>(
            &'a self,
            agent_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send + 'a>> {
            let ids = self.skill_ids.get(agent_id).cloned().unwrap_or_default();
            Box::pin(std::future::ready(ids))
        }
    }
}
