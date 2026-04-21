use std::future::Future;
use std::pin::Pin;

use async_nats::jetstream::{self, kv};
use tracing::warn;

const CONSOLE_AGENTS_BUCKET: &str = "CONSOLE_AGENTS";

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

// ── Types ─────────────────────────────────────────────────────────────────────

/// Agent configuration loaded from the console KV store.
pub struct AgentConfig {
    pub skill_ids: Vec<String>,
    /// System prompt configured in the console. `None` if not set.
    pub system_prompt: Option<String>,
    /// Model ID configured in the console (e.g. `"grok-3"`). `None` if not set.
    pub model_id: Option<String>,
}

// ── Trait ─────────────────────────────────────────────────────────────────────

pub trait AgentLoading: Send + Sync + 'static {
    fn load_config<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, AgentConfig>;
}

// ── Real implementation ───────────────────────────────────────────────────────

/// Reads the agent definition from the console KV store.
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
                warn!(
                    agent_id,
                    "agent not found in CONSOLE_AGENTS — no config injected"
                );
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

/// Parse an `AgentConfig` from the JSON value stored in CONSOLE_AGENTS.
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

impl AgentConfig {
    fn empty() -> Self {
        Self {
            skill_ids: vec![],
            system_prompt: None,
            model_id: None,
        }
    }
}

impl AgentLoading for AgentLoader {
    fn load_config<'a>(&'a self, agent_id: &'a str) -> BoxFuture<'a, AgentConfig> {
        Box::pin(self.load_config_impl(agent_id))
    }
}

// ── Mock (test only) ──────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    use super::{AgentConfig, AgentLoading};
    use std::collections::HashMap;

    pub struct MockAgentLoader {
        configs: HashMap<String, (Vec<String>, Option<String>, Option<String>)>,
    }

    impl MockAgentLoader {
        pub fn new() -> Self {
            Self {
                configs: HashMap::new(),
            }
        }

        pub fn insert(&mut self, agent_id: &str, skill_ids: Vec<String>) {
            self.configs
                .insert(agent_id.to_string(), (skill_ids, None, None));
        }

        pub fn insert_full(
            &mut self,
            agent_id: &str,
            skill_ids: Vec<String>,
            system_prompt: Option<String>,
            model_id: Option<String>,
        ) {
            self.configs
                .insert(agent_id.to_string(), (skill_ids, system_prompt, model_id));
        }
    }

    impl AgentLoading for MockAgentLoader {
        fn load_config<'a>(
            &'a self,
            agent_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = AgentConfig> + Send + 'a>> {
            let config = self
                .configs
                .get(agent_id)
                .map(|(ids, sp, mid)| AgentConfig {
                    skill_ids: ids.clone(),
                    system_prompt: sp.clone(),
                    model_id: mid.clone(),
                })
                .unwrap_or_else(AgentConfig::empty);
            Box::pin(std::future::ready(config))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> AgentConfig {
        let val: serde_json::Value = serde_json::from_str(json).unwrap();
        parse_agent_config(&val)
    }

    #[test]
    fn parse_full_config() {
        let cfg = parse(
            r#"{"skill_ids":["s1","s2"],"system_prompt":"Be helpful","model":{"id":"grok-4"}}"#,
        );
        assert_eq!(cfg.skill_ids, vec!["s1", "s2"]);
        assert_eq!(cfg.system_prompt.as_deref(), Some("Be helpful"));
        assert_eq!(cfg.model_id.as_deref(), Some("grok-4"));
    }

    #[test]
    fn parse_empty_strings_become_none() {
        let cfg = parse(r#"{"skill_ids":[],"system_prompt":"","model":{"id":""}}"#);
        assert!(cfg.skill_ids.is_empty());
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn parse_missing_fields_returns_defaults() {
        let cfg = parse("{}");
        assert!(cfg.skill_ids.is_empty());
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn parse_skill_ids_skips_non_strings() {
        let cfg = parse(r#"{"skill_ids":["ok", 42, null, "also-ok"]}"#);
        assert_eq!(cfg.skill_ids, vec!["ok", "also-ok"]);
    }

    #[test]
    fn parse_model_id_nested_under_model_key() {
        let cfg = parse(r#"{"model":{"id":"grok-3-mini"}}"#);
        assert_eq!(cfg.model_id.as_deref(), Some("grok-3-mini"));
    }

    #[test]
    fn agent_config_empty() {
        let cfg = AgentConfig::empty();
        assert!(cfg.skill_ids.is_empty());
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn mock_loader_returns_empty_for_unknown_agent() {
        use mock::MockAgentLoader;
        let loader = MockAgentLoader::new();
        let cfg = futures::executor::block_on(loader.load_config("unknown"));
        assert!(cfg.skill_ids.is_empty());
        assert!(cfg.system_prompt.is_none());
    }

    #[test]
    fn mock_loader_insert_returns_skill_ids() {
        use mock::MockAgentLoader;
        let mut loader = MockAgentLoader::new();
        loader.insert("agent1", vec!["sk-a".into(), "sk-b".into()]);
        let cfg = futures::executor::block_on(loader.load_config("agent1"));
        assert_eq!(cfg.skill_ids, vec!["sk-a", "sk-b"]);
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn mock_loader_insert_full_returns_all_fields() {
        use mock::MockAgentLoader;
        let mut loader = MockAgentLoader::new();
        loader.insert_full(
            "agent2",
            vec!["sk-x".into()],
            Some("You are an expert.".into()),
            Some("grok-4".into()),
        );
        let cfg = futures::executor::block_on(loader.load_config("agent2"));
        assert_eq!(cfg.skill_ids, vec!["sk-x"]);
        assert_eq!(cfg.system_prompt.as_deref(), Some("You are an expert."));
        assert_eq!(cfg.model_id.as_deref(), Some("grok-4"));
    }
}
