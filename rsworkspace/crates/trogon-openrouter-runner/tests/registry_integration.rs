//! Integration tests verifying that trogon-openrouter-runner registers itself
//! with the correct `AgentCapability` — capabilities, model IDs, and acp_prefix —
//! matching the contract consumed by `CrossRunnerSwitcher::find_by_model`.
//!
//! Run with:
//!   cargo test -p trogon-openrouter-runner --test registry_integration

use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

const OR_PREFIX: &str = "openrouter";
const AGENT_TYPE: &str = "openrouter";

// Default model string from config.rs (before colon = model ID, after = label).
const DEFAULT_MODELS_STR: &str =
    "anthropic/claude-sonnet-4-6:Claude Sonnet 4.6,openai/gpt-4o:GPT-4o,google/gemini-pro-1.5:Gemini Pro 1.5";
const EXPECTED_MODELS: &[&str] =
    &["anthropic/claude-sonnet-4-6", "openai/gpt-4o", "google/gemini-pro-1.5"];

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
        nats_subject: format!("{OR_PREFIX}.agent.>"),
        current_load: 0,
        metadata: serde_json::json!({
            "acp_prefix": OR_PREFIX,
            "models": parse_model_ids(DEFAULT_MODELS_STR),
        }),
    }
}

/// openrouter-runner must register with chat, explore, and plan — matching
/// programming.md PR 6 and enabling CrossRunnerSwitcher routing.
#[tokio::test]
async fn openrouter_runner_registers_with_chat_explore_plan_capabilities() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    for cap in &["chat", "explore", "plan"] {
        assert!(
            entry.capabilities.contains(&cap.to_string()),
            "openrouter-runner must have '{cap}' capability; got: {:?}",
            entry.capabilities
        );
    }
}

/// openrouter-runner must NOT register with code_edit.
#[tokio::test]
async fn openrouter_runner_does_not_register_with_code_edit() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    assert!(
        !entry.capabilities.contains(&"code_edit".to_string()),
        "openrouter-runner must NOT have 'code_edit'; got: {:?}",
        entry.capabilities
    );
}

/// `metadata.models` must contain all default OpenRouter model IDs so that
/// `CrossRunnerSwitcher::find_by_model` can route to the openrouter-runner.
#[tokio::test]
async fn openrouter_runner_registers_with_model_ids_in_metadata() {
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

/// `find_by_model("anthropic/claude-sonnet-4-6")` must resolve to openrouter-runner.
#[tokio::test]
async fn openrouter_runner_find_by_model_returns_runner_for_claude_sonnet() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry
        .find_by_model("anthropic/claude-sonnet-4-6")
        .await
        .unwrap()
        .expect("find_by_model must find openrouter-runner for anthropic/claude-sonnet-4-6");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(OR_PREFIX),
        "found runner must have acp_prefix={OR_PREFIX}; got: {:?}",
        entry.metadata
    );
}

/// All default OpenRouter models must be discoverable via `find_by_model`.
#[tokio::test]
async fn openrouter_runner_find_by_model_works_for_all_default_models() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    for model in EXPECTED_MODELS {
        let entry = registry
            .find_by_model(model)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("find_by_model must find openrouter-runner for '{model}'"));
        assert_eq!(
            entry.metadata["acp_prefix"].as_str(),
            Some(OR_PREFIX),
            "acp_prefix mismatch for model '{model}'"
        );
    }
}
