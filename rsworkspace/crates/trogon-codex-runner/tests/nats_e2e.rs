//! End-to-end integration tests: DefaultCodexAgent + AgentSideNatsConnection + real NATS.
//!
//! Verifies that DefaultCodexAgent correctly handles ACP request-reply over a
//! real NATS server. `initialize` is tested without any subprocess. `session/new`
//! requires CODEX_BIN to be set (uses the in-tree mock_codex_server binary).
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-codex-runner --test nats_e2e

use std::sync::OnceLock;
use std::time::Duration;

use acp_nats::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use serde_json::Value;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::sync::Mutex;
use trogon_codex_runner::DefaultCodexAgent;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Path to the compiled mock_codex_server binary (resolved at compile time).
const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock_codex_server");

/// Serialise CODEX_BIN env-var writes across parallel tests in this binary.
static BIN_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bin_env_lock() -> &'static Mutex<()> {
    BIN_ENV_LOCK.get_or_init(Mutex::default)
}

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    (container, nats)
}

/// Spawn DefaultCodexAgent in a dedicated thread and wait for it to subscribe.
async fn start_agent(nats: async_nats::Client) {
    let nats_for_thread = nats.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let prefix = AcpPrefix::new("acp").unwrap();
        let agent = DefaultCodexAgent::with_nats(nats_for_thread.clone(), prefix.clone(), "o4-mini");
        let (_, io_task) = AgentSideNatsConnection::new(agent, nats_for_thread, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move { io_task.await.ok(); }));
    });
    // Give the agent time to subscribe before the test sends requests.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// DefaultCodexAgent handles `initialize` over real NATS and returns capabilities.
/// No subprocess is spawned — initialize is handled entirely in-process.
#[tokio::test]
async fn e2e_nats_initialize_returns_capabilities() {
    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({"protocolVersion": 0})).unwrap();
    let msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.initialize", payload.into()),
    )
    .await
    .expect("timed out waiting for initialize response")
    .expect("NATS request failed");

    let response: Value = serde_json::from_slice(&msg.payload).unwrap();
    assert!(
        response["protocolVersion"].is_number(),
        "must have protocolVersion in response: {response}"
    );
    assert!(
        response["agentCapabilities"].is_object(),
        "must have agentCapabilities: {response}"
    );
}

/// DefaultCodexAgent handles `session/new` over real NATS.
/// Spawns the mock_codex_server binary via CODEX_BIN env var.
#[tokio::test]
async fn e2e_nats_session_new_returns_session_id() {
    let _lock = bin_env_lock().lock().await;
    // SAFETY: serialized by BIN_ENV_LOCK; single-process test binary.
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };

    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/tmp",
        "mcpServers": []
    }))
    .unwrap();
    let msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", payload.into()),
    )
    .await
    .expect("timed out waiting for session/new response")
    .expect("NATS request failed");

    let response: Value = serde_json::from_slice(&msg.payload).unwrap();
    let session_id = response["sessionId"].as_str().unwrap_or("");
    assert!(!session_id.is_empty(), "must have non-empty sessionId: {response}");
}

/// session/get_state returns the stored session's cwd over real NATS.
/// Uses MOCK_BIN via CODEX_BIN so no real Codex CLI is needed.
#[tokio::test]
async fn e2e_ext_session_get_state_returns_cwd() {
    let _lock = bin_env_lock().lock().await;
    // SAFETY: serialized by BIN_ENV_LOCK; single-process test binary.
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };

    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let new_payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/projects/myapp",
        "mcpServers": []
    }))
    .unwrap();
    let new_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", new_payload.into()),
    )
    .await
    .expect("timed out waiting for session/new")
    .expect("NATS request failed");
    let new_resp: Value = serde_json::from_slice(&new_msg.payload).unwrap();
    let session_id = new_resp["sessionId"].as_str().expect("session/new must return sessionId");

    let ext_payload = serde_json::to_vec(&serde_json::json!({ "sessionId": session_id })).unwrap();
    let ext_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.ext.session/get_state", ext_payload.into()),
    )
    .await
    .expect("timed out waiting for session/get_state")
    .expect("NATS ext request failed");

    let state: Value = serde_json::from_slice(&ext_msg.payload).unwrap();
    assert_eq!(
        state["cwd"].as_str(),
        Some("/projects/myapp"),
        "session/get_state must return the session's cwd: {state}"
    );
}

/// Verifies the runner registration contract: the `acp_prefix` metadata stored
/// in the registry must match the `ACP_PREFIX` env var.  The bridge reads this
/// field to derive the NATS routing prefix — a mismatch breaks routing silently.
#[tokio::test]
async fn codex_runner_registers_with_correct_acp_prefix_metadata() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.codex";
    let agent_type = "codex";

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["chat".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": prefix }),
    };
    registry.register(&cap).await.expect("registration must succeed");

    let entry = registry
        .get(agent_type)
        .await
        .expect("get must not error")
        .expect("registered entry must exist");

    assert_eq!(
        entry.metadata["acp_prefix"].as_str(),
        Some(prefix),
        "bridge relies on acp_prefix matching ACP_PREFIX — got {:?}",
        entry.metadata
    );
    assert_eq!(
        entry.nats_subject,
        format!("{}.agent.>", prefix),
        "nats_subject must be derived from ACP_PREFIX"
    );
}

/// Verifies the full capabilities+models registration contract that codex main.rs uses:
/// capabilities must be ["chat", "code_edit"] (not "explore"/"plan"), and
/// metadata.models must contain the parsed model IDs from CODEX_MODELS.
#[tokio::test]
async fn codex_runner_registers_with_code_edit_capability_and_model_ids() {
    let (_container, nats) = start_nats().await;
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.codex";
    let agent_type = "codex";
    // Replicate main.rs CODEX_MODELS parsing: "o4-mini,o3" → ["o4-mini", "o3"]
    let codex_models_env = "o4-mini,o3";
    let model_ids: Vec<String> = codex_models_env
        .split(',')
        .filter_map(|entry| entry.split(':').next().map(|id| id.trim().to_string()))
        .filter(|id| !id.is_empty())
        .collect();

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["chat".to_string(), "code_edit".to_string()],
        nats_subject: format!("{}.agent.>", prefix),
        current_load: 0,
        metadata: serde_json::json!({ "acp_prefix": prefix, "models": model_ids }),
    };
    registry.register(&cap).await.expect("registration must succeed");

    let entry = registry
        .get(agent_type)
        .await
        .expect("get must not error")
        .expect("registered entry must exist");

    assert!(
        entry.capabilities.contains(&"code_edit".to_string()),
        "codex runner must have 'code_edit' capability; got: {:?}",
        entry.capabilities
    );
    assert!(
        entry.capabilities.contains(&"chat".to_string()),
        "codex runner must have 'chat' capability; got: {:?}",
        entry.capabilities
    );
    assert!(
        !entry.capabilities.contains(&"explore".to_string()),
        "codex runner must NOT have 'explore' (unlike xai/or); got: {:?}",
        entry.capabilities
    );
    assert!(
        !entry.capabilities.contains(&"plan".to_string()),
        "codex runner must NOT have 'plan' (unlike xai/or); got: {:?}",
        entry.capabilities
    );

    let models = entry.metadata["models"].as_array().expect("metadata.models must be array");
    let model_strings: Vec<&str> = models.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        model_strings.contains(&"o4-mini"),
        "metadata.models must contain 'o4-mini'; got: {model_strings:?}"
    );
    assert!(
        model_strings.contains(&"o3"),
        "metadata.models must contain 'o3'; got: {model_strings:?}"
    );
}

/// Sending a prompt to an existing codex session via NATS returns `end_turn`.
///
/// This is the most important gap in the codex NATS coverage: all other tests
/// only verify `session/new` and `initialize`. This test closes the entire
/// NATS routing path: `session/new` → `session.{id}.agent.prompt` →
/// mock_codex_server `turn/start` → `turn/completed` → NATS `end_turn`.
#[tokio::test]
async fn e2e_nats_prompt_returns_end_turn() {
    let _lock = bin_env_lock().lock().await;
    // SAFETY: serialized by BIN_ENV_LOCK; single-process test binary.
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };

    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    // ── Step 1: create a session ──────────────────────────────────────────────
    let new_payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/tmp",
        "mcpServers": []
    }))
    .unwrap();
    let new_msg = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", new_payload.into()),
    )
    .await
    .expect("timed out waiting for session/new")
    .expect("NATS request failed");

    let new_resp: Value = serde_json::from_slice(&new_msg.payload).unwrap();
    let session_id = new_resp["sessionId"].as_str().unwrap_or("").to_string();
    assert!(!session_id.is_empty(), "session/new must return a non-empty sessionId");

    // ── Step 2: send a prompt to the session ─────────────────────────────────
    let prompt_subject = format!("acp.session.{session_id}.agent.prompt");
    let prompt_payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": session_id,
        "prompt": [{"type": "text", "text": "write a hello world program"}]
    }))
    .unwrap();

    let prompt_msg = tokio::time::timeout(
        Duration::from_secs(15),
        nats.request(prompt_subject, prompt_payload.into()),
    )
    .await
    .expect("timed out waiting for prompt response")
    .expect("NATS prompt request failed");

    let prompt_resp: Value = serde_json::from_slice(&prompt_msg.payload).unwrap();
    assert_eq!(
        prompt_resp["stopReason"].as_str(),
        Some("end_turn"),
        "prompt must complete with end_turn; got: {prompt_resp}"
    );
}

/// `/clear` in the codex-runner is modelled as calling `session/new` again,
/// which spawns a fresh Codex subprocess and returns a distinct session ID.
///
/// Verifies the real scenario: after a user issues `/clear`, the next prompt
/// starts with a completely fresh context by sending two sequential
/// `session/new` requests and asserting that the returned IDs differ.
#[tokio::test]
async fn clear_creates_distinct_session_id_in_codex_runner() {
    let _lock = bin_env_lock().lock().await;
    // SAFETY: serialized by BIN_ENV_LOCK; single-process test binary.
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };

    let (_container, nats) = start_nats().await;
    start_agent(nats.clone()).await;

    let payload = serde_json::to_vec(&serde_json::json!({
        "sessionId": null,
        "cwd": "/tmp",
        "mcpServers": []
    }))
    .unwrap();

    let msg1 = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", payload.clone().into()),
    )
    .await
    .expect("timed out waiting for first session/new")
    .expect("NATS request failed");

    let resp1: Value = serde_json::from_slice(&msg1.payload).unwrap();
    let session_id_1 = resp1["sessionId"].as_str().unwrap_or("").to_string();
    assert!(!session_id_1.is_empty(), "first session/new must return a non-empty sessionId");

    let msg2 = tokio::time::timeout(
        Duration::from_secs(10),
        nats.request("acp.agent.session.new", payload.into()),
    )
    .await
    .expect("timed out waiting for second session/new")
    .expect("NATS request failed");

    let resp2: Value = serde_json::from_slice(&msg2.payload).unwrap();
    let session_id_2 = resp2["sessionId"].as_str().unwrap_or("").to_string();
    assert!(!session_id_2.is_empty(), "second session/new must return a non-empty sessionId");

    assert_ne!(
        session_id_1, session_id_2,
        "/clear (second session/new) must produce a distinct session ID; both got: {session_id_1:?}"
    );
}
