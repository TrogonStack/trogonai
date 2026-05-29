//! Integration tests verifying that trogon-acp-runner registers itself with
//! the correct `AgentCapability` — capabilities, model IDs, and acp_prefix —
//! matching the contract consumed by `CrossRunnerSwitcher::find_by_model`.
//!
//! These tests replicate what `main.rs` does at startup so that regressions
//! in the registration contract are caught before they silently break routing.
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test registry_integration

use trogon_registry::{AgentCapability, MockRegistryStore, Registry};

const ACP_PREFIX: &str = "acp";
const AGENT_TYPE: &str = "claude";
const MODELS: &[&str] = &[
    "claude-opus-4-6",
    "claude-sonnet-4-6",
    "claude-haiku-4-5-20251001",
];

/// Builds the same `AgentCapability` that `main.rs` constructs at startup.
fn make_cap() -> AgentCapability {
    AgentCapability {
        agent_type: AGENT_TYPE.to_string(),
        capabilities: vec!["chat".to_string(), "code_edit".to_string()],
        nats_subject: format!("{ACP_PREFIX}.agent.>"),
        current_load: 0,
        metadata: serde_json::json!({
            "acp_prefix": ACP_PREFIX,
            "models": MODELS,
        }),
    }
}

/// ACP runner registers with `chat` and `code_edit` capabilities, matching the
/// contract for a full-featured programming assistant (not just "explore"/"plan"
/// like xai/openrouter).
#[tokio::test]
async fn acp_runner_registers_with_chat_and_code_edit_capabilities() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    assert!(
        entry.capabilities.contains(&"chat".to_string()),
        "acp-runner must have 'chat' capability; got: {:?}",
        entry.capabilities
    );
    assert!(
        entry.capabilities.contains(&"code_edit".to_string()),
        "acp-runner must have 'code_edit' capability; got: {:?}",
        entry.capabilities
    );
}

/// ACP runner does NOT register with explore/plan — unlike xai/openrouter.
#[tokio::test]
async fn acp_runner_does_not_register_with_explore_or_plan() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    assert!(
        !entry.capabilities.contains(&"explore".to_string()),
        "acp-runner must NOT have 'explore'; got: {:?}",
        entry.capabilities
    );
    assert!(
        !entry.capabilities.contains(&"plan".to_string()),
        "acp-runner must NOT have 'plan'; got: {:?}",
        entry.capabilities
    );
}

/// `metadata.models` must contain all expected Claude model IDs so that
/// `CrossRunnerSwitcher::find_by_model` can route to the ACP runner.
#[tokio::test]
async fn acp_runner_registers_with_claude_model_ids_in_metadata() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    let models = entry.metadata["models"]
        .as_array()
        .expect("metadata.models must be an array");
    let model_ids: Vec<&str> = models.iter().filter_map(|v| v.as_str()).collect();

    for expected in MODELS {
        assert!(
            model_ids.contains(expected),
            "metadata.models must contain '{expected}'; got: {model_ids:?}"
        );
    }
}

/// `find_by_model("claude-opus-4-6")` must resolve to the ACP runner and
/// expose the correct `acp_prefix` — the field CrossRunnerSwitcher reads to
/// build the NATS routing subject.
#[tokio::test]
async fn acp_runner_find_by_model_returns_runner_for_claude_opus() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry
        .find_by_model("claude-opus-4-6")
        .await
        .unwrap()
        .expect("find_by_model must find acp-runner for claude-opus-4-6");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(ACP_PREFIX),
        "found runner must have acp_prefix={ACP_PREFIX}; got: {:?}",
        entry.metadata
    );
}

/// `nats_subject` must be `{acp_prefix}.agent.>` — the pattern the ACP
/// NATS bridge uses when subscribed to route requests from the gateway.
#[tokio::test]
async fn acp_runner_nats_subject_matches_acp_prefix() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    let entry = registry.get(AGENT_TYPE).await.unwrap().unwrap();
    assert_eq!(
        entry.nats_subject,
        format!("{ACP_PREFIX}.agent.>"),
        "nats_subject must be derived from ACP_PREFIX"
    );
}

/// ACP runner must also be discoverable by sonnet and haiku model IDs.
#[tokio::test]
async fn acp_runner_find_by_model_works_for_all_claude_models() {
    let registry = Registry::new(MockRegistryStore::new());
    registry.register(&make_cap()).await.unwrap();

    for model in MODELS {
        let entry = registry
            .find_by_model(model)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("find_by_model must find acp-runner for '{model}'"));
        assert_eq!(
            entry.metadata["acp_prefix"].as_str(),
            Some(ACP_PREFIX),
            "acp_prefix mismatch for model '{model}'"
        );
    }
}
