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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> AgentConfig {
        let val: serde_json::Value = serde_json::from_str(json).unwrap();
        parse_agent_config(&val)
    }

    #[test]
    fn full_config_parsed_correctly() {
        let cfg = parse(r#"{"skill_ids":["s1","s2"],"system_prompt":"Be brief.","model":{"id":"gpt-4o"}}"#);
        assert_eq!(cfg.skill_ids, vec!["s1", "s2"]);
        assert_eq!(cfg.system_prompt.as_deref(), Some("Be brief."));
        assert_eq!(cfg.model_id.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn missing_skill_ids_defaults_to_empty() {
        let cfg = parse(r#"{"system_prompt":"hi","model":{"id":"x"}}"#);
        assert!(cfg.skill_ids.is_empty());
    }

    #[test]
    fn empty_skill_ids_array() {
        let cfg = parse(r#"{"skill_ids":[]}"#);
        assert!(cfg.skill_ids.is_empty());
    }

    #[test]
    fn skill_ids_with_non_string_entries_are_skipped() {
        let cfg = parse(r#"{"skill_ids":["ok",42,null,true,"also-ok"]}"#);
        assert_eq!(cfg.skill_ids, vec!["ok", "also-ok"]);
    }

    #[test]
    fn empty_system_prompt_becomes_none() {
        let cfg = parse(r#"{"system_prompt":""}"#);
        assert!(cfg.system_prompt.is_none());
    }

    #[test]
    fn missing_system_prompt_is_none() {
        let cfg = parse(r#"{}"#);
        assert!(cfg.system_prompt.is_none());
    }

    #[test]
    fn empty_model_id_becomes_none() {
        let cfg = parse(r#"{"model":{"id":""}}"#);
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn missing_model_key_is_none() {
        let cfg = parse(r#"{"skill_ids":[]}"#);
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn model_not_an_object_is_none() {
        let cfg = parse(r#"{"model":"gpt-4o"}"#);
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn model_without_id_field_is_none() {
        let cfg = parse(r#"{"model":{"name":"gpt-4o"}}"#);
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn model_id_null_is_none() {
        let cfg = parse(r#"{"model":{"id":null}}"#);
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn model_id_non_string_is_none() {
        let cfg = parse(r#"{"model":{"id":42}}"#);
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn extra_fields_are_ignored() {
        let cfg = parse(r#"{"skill_ids":["x"],"unknown_key":"value","model":{"id":"m1","extra":true}}"#);
        assert_eq!(cfg.skill_ids, vec!["x"]);
        assert_eq!(cfg.model_id.as_deref(), Some("m1"));
    }

    #[test]
    fn empty_string_skill_id_is_kept_in_list() {
        // Empty string skill IDs are not filtered — the caller decides what to do with them.
        let cfg = parse(r#"{"skill_ids":["","valid-id"]}"#);
        assert_eq!(cfg.skill_ids, vec!["", "valid-id"]);
    }

    #[test]
    fn system_prompt_whitespace_only_is_kept() {
        // Only strictly empty string is filtered; whitespace-only prompts are preserved.
        let cfg = parse(r#"{"system_prompt":"   "}"#);
        assert_eq!(cfg.system_prompt.as_deref(), Some("   "));
    }

    #[test]
    fn all_optional_fields_absent_returns_all_defaults() {
        let cfg = parse(r#"{}"#);
        assert!(cfg.skill_ids.is_empty());
        assert!(cfg.system_prompt.is_none());
        assert!(cfg.model_id.is_none());
    }

    #[test]
    fn model_id_whitespace_only_is_kept() {
        // Only strictly empty model id is filtered; whitespace is kept.
        let cfg = parse(r#"{"model":{"id":"  "}}"#);
        assert_eq!(cfg.model_id.as_deref(), Some("  "));
    }

    #[test]
    fn skill_ids_null_array_falls_back_to_empty() {
        // skill_ids: null → treat as missing → empty vec.
        let cfg = parse(r#"{"skill_ids":null}"#);
        assert!(cfg.skill_ids.is_empty());
    }
}
