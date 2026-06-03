//! Integration tests verifying that trogon-codex-runner registers itself with
//! the correct `AgentCapability` — capabilities, model IDs, and acp_prefix —
//! matching the contract consumed by `CrossRunnerSwitcher::find_by_model`.
//!
//! Run with:
//!   cargo test -p trogon-codex-runner --test registry_integration

use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

const CODEX_PREFIX: &str = "codex";
const AGENT_TYPE: &str = "codex";

// Default model string from main.rs (before colon = model ID, after = label).
const DEFAULT_MODELS_STR: &str = "o4-mini:o4-mini,o3:o3,gpt-4o:GPT-4o";
const EXPECTED_MODELS: &[&str] = &["o4-mini", "o3", "gpt-4o"];

fn parse_model_ids(models_str: &str) -> Vec<String> {
    models_str
        .split(',')
        .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
        .collect()
}

fn make_cap() -> AgentCapability {
    AgentCapability {
        agent_type: AGENT_TYPE.to_string(),
        capabilities: vec!["chat".to_string(), "code_edit".to_string()],
        nats_subject: format!("{CODEX_PREFIX}.agent.>"),
        current_load: 0,
        metadata: serde_json::json!({
            "acp_prefix": CODEX_PREFIX,
            "models": parse_model_ids(DEFAULT_MODELS_STR),
        }),
    }
}

/// codex-runner must register with chat and code_edit — reflecting its
/// native tool-execution capability via the Codex CLI subprocess.
#[tokio::test]
async fn codex_runner_registers_with_chat_and_code_edit_capabilities() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    for cap in &["chat", "code_edit"] {
        assert!(
            entry.capabilities.contains(&cap.to_string()),
            "codex-runner must have '{cap}' capability; got: {:?}",
            entry.capabilities
        );
    }
}

/// codex-runner must NOT register with explore/plan — it is a full execution
/// runner, not a read-only or planning runner.
#[tokio::test]
async fn codex_runner_does_not_register_with_explore_or_plan() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    for cap in &["explore", "plan"] {
        assert!(
            !entry.capabilities.contains(&cap.to_string()),
            "codex-runner must NOT have '{cap}'; got: {:?}",
            entry.capabilities
        );
    }
}

/// `metadata.models` must contain all default Codex model IDs so that
/// `CrossRunnerSwitcher::find_by_model` can route to the codex-runner.
#[tokio::test]
async fn codex_runner_registers_with_model_ids_in_metadata() {
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

/// `find_by_model("o4-mini")` must resolve to the codex-runner and expose
/// the correct `acp_prefix` used by CrossRunnerSwitcher.
#[tokio::test]
async fn codex_runner_find_by_model_returns_codex_for_o4_mini() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry
        .find_by_model("o4-mini")
        .await
        .unwrap()
        .expect("find_by_model must find codex-runner for o4-mini");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(CODEX_PREFIX),
        "found runner must have acp_prefix={CODEX_PREFIX}; got: {:?}",
        entry.metadata
    );
}

/// All default Codex models must be discoverable via `find_by_model`.
#[tokio::test]
async fn codex_runner_find_by_model_works_for_all_default_models() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    for model in EXPECTED_MODELS {
        let entry = registry
            .find_by_model(model)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("find_by_model must find codex-runner for '{model}'"));
        assert_eq!(
            entry.metadata["acp_prefix"].as_str(),
            Some(CODEX_PREFIX),
            "acp_prefix mismatch for model '{model}'"
        );
    }
}
