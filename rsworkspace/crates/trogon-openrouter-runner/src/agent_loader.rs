use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};
use tracing::warn;

const CONSOLE_AGENTS_BUCKET: &str = "CONSOLE_AGENTS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct AgentConfig {
    pub skill_ids: Vec<String>,
    pub system_prompt: Option<String>,
    pub model_id: Option<String>,
}

impl AgentConfig {
    fn empty() -> Self {
        Self {
            skill_ids: vec![],
            system_prompt: None,
            model_id: None,
        }
    }
}

pub trait AgentLoading: Send + Sync + 'static {
    fn load_config<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, AgentConfig>;
}

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

    async fn load_config_impl(&self, agent_id: &str) -> AgentConfig {
        let bytes = match self.agents_kv.get(agent_id).await {
            Ok(Some(b)) => b,
            Ok(None) => {
                warn!(agent_id, "agent not found in CONSOLE_AGENTS — no config injected");
                return AgentConfig::empty();
            }
            Err(e) => {
                warn!(agent_id, error = %e, "failed to read CONSOLE_AGENTS");
                return AgentConfig::empty();
            }
        };

        let Ok(val) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            warn!(agent_id, "CONSOLE_AGENTS entry is not valid JSON");
            return AgentConfig::empty();
        };

        parse_agent_config(&val)
    }
}

pub(crate) fn parse_agent_config(val: &serde_json::Value) -> AgentConfig {
    let skill_ids = val["skill_ids"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let system_prompt = val["system_prompt"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let model_id = val["model"]["id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    AgentConfig {
        skill_ids,
        system_prompt,
        model_id,
    }
}

impl AgentLoading for AgentLoader {
    fn load_config<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, AgentConfig> {
        Box::pin(self.load_config_impl(agent_id))
    }
}
