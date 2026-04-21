use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};
use tracing::warn;

const CONSOLE_AGENTS_BUCKET: &str = "CONSOLE_AGENTS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Agent configuration read from a single `CONSOLE_AGENTS` KV entry.
#[derive(Debug, Default)]
pub struct AgentConfig {
    pub skill_ids: Vec<String>,
    /// `model.id` field from the console agent definition.
    pub model_id: Option<String>,
    /// `system_prompt` field from the console agent definition.
    pub system_prompt: Option<String>,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait AgentLoading: Send + Sync + 'static {
    fn get_agent_config<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, AgentConfig>;
}

// ── Real implementation ───────────────────────────────────────────────────────

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

    async fn get_agent_config_impl(&self, agent_id: &str) -> AgentConfig {
        match self.agents_kv.get(agent_id).await {
            Ok(Some(bytes)) => {
                let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    warn!(agent_id, "CONSOLE_AGENTS entry is not valid JSON");
                    return AgentConfig::default();
                };
                let skill_ids = val["skill_ids"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let model_id = val["model"]["id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                let system_prompt = val["system_prompt"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string());
                AgentConfig { skill_ids, model_id, system_prompt }
            }
            Ok(None) => {
                warn!(agent_id, "agent not found in CONSOLE_AGENTS");
                AgentConfig::default()
            }
            Err(e) => {
                warn!(agent_id, error = %e, "Failed to read CONSOLE_AGENTS");
                AgentConfig::default()
            }
        }
    }
}

impl AgentLoading for AgentLoader {
    fn get_agent_config<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, AgentConfig> {
        Box::pin(self.get_agent_config_impl(agent_id))
    }
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(feature = "test-helpers")]
pub mod mock {
    use super::{AgentConfig, AgentLoading};
    use std::collections::HashMap;

    pub struct MockAgentLoader {
        configs: HashMap<String, AgentConfig>,
    }

    impl MockAgentLoader {
        pub fn new() -> Self {
            Self { configs: HashMap::new() }
        }

        pub fn insert(
            &mut self,
            agent_id: &str,
            skill_ids: Vec<String>,
            model_id: Option<String>,
            system_prompt: Option<String>,
        ) {
            self.configs.insert(
                agent_id.to_string(),
                AgentConfig { skill_ids, model_id, system_prompt },
            );
        }
    }

    impl AgentLoading for MockAgentLoader {
        fn get_agent_config<'a>(
            &'a self,
            agent_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>> {
            let config = self.configs.get(agent_id).map(|c| AgentConfig {
                skill_ids: c.skill_ids.clone(),
                model_id: c.model_id.clone(),
                system_prompt: c.system_prompt.clone(),
            }).unwrap_or_default();
            Box::pin(std::future::ready(config))
        }
    }
}
