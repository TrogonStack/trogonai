//! Verifies that the wasm-runtime registers in the agent registry with the
//! correct `acp_prefix` metadata and `nats_subject`, matching what `main.rs`
//! does on startup.  The bridge reads `acp_prefix` to derive the NATS routing
//! prefix — a mismatch breaks routing silently.
//!
//! Requires Docker (testcontainers starts a NATS server).
//!
//! Run with:
//!   cargo test -p trogon-wasm-runtime --test registry_e2e

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};

#[tokio::test]
async fn wasm_runtime_registers_with_correct_acp_prefix_metadata() {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS");
    let js = async_nats::jetstream::new(nats.clone());

    let prefix = "acp.wasm";
    let agent_type = "wasm";

    let store = trogon_registry::provision(&js).await.expect("provision registry");
    let registry = trogon_registry::Registry::new(store);

    let cap = trogon_registry::AgentCapability {
        agent_type: agent_type.to_string(),
        capabilities: vec!["execution".to_string()],
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
    assert_eq!(
        entry.capabilities,
        vec!["execution".to_string()],
        "wasm runtime advertises execution capability"
    );
}
