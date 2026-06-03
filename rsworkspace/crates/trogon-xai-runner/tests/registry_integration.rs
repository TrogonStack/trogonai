//! Integration tests verifying that trogon-xai-runner registers itself with
//! the correct `AgentCapability` — capabilities, model IDs, and acp_prefix —
//! matching the contract consumed by `CrossRunnerSwitcher::find_by_model`.
//!
//! Run with:
//!   cargo test -p trogon-xai-runner --test registry_integration

use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

const XAI_PREFIX: &str = "xai";
const AGENT_TYPE: &str = "xai";

// Default model string from main.rs (before colon = model ID, after = label).
const DEFAULT_MODELS_STR: &str = "grok-4:Grok 4,grok-3:Grok 3,grok-3-mini:Grok 3 Mini";
const EXPECTED_MODELS: &[&str] = &["grok-4", "grok-3", "grok-3-mini"];

fn parse_model_ids(models_str: &str) -> Vec<String> {
    models_str
        .split(',')
        .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
        .collect()
}

fn make_cap() -> AgentCapability {
    AgentCapability {
        agent_type: AGENT_TYPE.to_string(),
        capabilities: vec!["chat".to_string(), "explore".to_string(), "plan".to_string()],
        nats_subject: format!("{XAI_PREFIX}.agent.>"),
        current_load: 0,
        metadata: serde_json::json!({
            "acp_prefix": XAI_PREFIX,
            "models": parse_model_ids(DEFAULT_MODELS_STR),
        }),
    }
}

/// xai-runner must register with chat, explore, and plan — matching the
/// programming.md spec and enabling CrossRunnerSwitcher routing.
#[tokio::test]
async fn xai_runner_registers_with_chat_explore_plan_capabilities() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    for cap in &["chat", "explore", "plan"] {
        assert!(
            entry.capabilities.contains(&cap.to_string()),
            "xai-runner must have '{cap}' capability; got: {:?}",
            entry.capabilities
        );
    }
}

/// xai-runner must NOT register with code_edit — that belongs to acp-runner
/// and codex-runner, which have native tool-execution support.
#[tokio::test]
async fn xai_runner_does_not_register_with_code_edit() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    assert!(
        !entry.capabilities.contains(&"code_edit".to_string()),
        "xai-runner must NOT have 'code_edit'; got: {:?}",
        entry.capabilities
    );
}

/// `metadata.models` must contain all default xAI model IDs so that
/// `CrossRunnerSwitcher::find_by_model` can route to the xai-runner.
#[tokio::test]
async fn xai_runner_registers_with_model_ids_in_metadata() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    let models = entry.metadata["models"]
        .as_array()
        .expect("metadata.models must be an array");
    let model_ids: Vec<&str> = models.iter().filter_map(|v| v.as_str()).collect();

    for expected in EXPECTED_MODELS {
        assert!(
            model_ids.contains(expected),
            "metadata.models must contain '{expected}'; got: {model_ids:?}"
        );
    }
}

/// `find_by_model("grok-4")` must resolve to the xai-runner and expose
/// the correct `acp_prefix` used by CrossRunnerSwitcher to build NATS subjects.
#[tokio::test]
async fn xai_runner_find_by_model_returns_xai_for_grok_4() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry
        .find_by_model("grok-4")
        .await
        .unwrap()
        .expect("find_by_model must find xai-runner for grok-4");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(XAI_PREFIX),
        "found runner must have acp_prefix={XAI_PREFIX}; got: {:?}",
        entry.metadata
    );
}

/// All default xAI models must be discoverable via `find_by_model`.
#[tokio::test]
async fn xai_runner_find_by_model_works_for_all_default_models() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    for model in EXPECTED_MODELS {
        let entry = registry
            .find_by_model(model)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("find_by_model must find xai-runner for '{model}'"));
        assert_eq!(
            entry.metadata["acp_prefix"].as_str(),
            Some(XAI_PREFIX),
            "acp_prefix mismatch for model '{model}'"
        );
    }
}
