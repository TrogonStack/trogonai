//! Integration tests for `RpcServer::handle_set_session_model`.
//!
//! Requires Docker (testcontainers starts a NATS server with JetStream).
//!
//! Run with:
//!   cargo test -p trogon-acp-runner --test rpc_server_integration

use std::sync::Arc;
use std::time::Duration;

use agent_client_protocol::{SetSessionModelRequest, SetSessionModelResponse};
use async_nats::jetstream;
use bytes::Bytes;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt};
use tokio::sync::RwLock;
use trogon_acp_runner::{RpcServer, SessionState, SessionStore};

async fn start_nats() -> (ContainerAsync<Nats>, async_nats::Client, jetstream::Context) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    let nats = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS");
    let js = jetstream::new(nats.clone());
    (container, nats, js)
}

/// Start an RpcServer and return the store for inspection.
async fn start_rpc_server(
    nats: async_nats::Client,
    js: jetstream::Context,
    prefix: &str,
) -> SessionStore {
    let store = SessionStore::open(&js).await.unwrap();
    let store_clone = store.clone();
    let gateway_config = Arc::new(RwLock::new(None));
    let server = RpcServer::new(nats, store_clone, prefix, gateway_config);
    tokio::spawn(async move { server.run().await });
    // Give the server time to set up subscriptions.
    tokio::time::sleep(Duration::from_millis(50)).await;
    store
}

// ── set_session_model ─────────────────────────────────────────────────────────

#[tokio::test]
async fn set_session_model_updates_model_in_store() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    // Pre-create a session in the store.
    store
        .save("sess-model-1", &SessionState::default())
        .await
        .unwrap();

    let request = SetSessionModelRequest::new("sess-model-1", "claude-opus-4");
    let payload = serde_json::to_vec(&request).unwrap();
    let reply = nats
        .request(
            "acp.sess-model-1.agent.session.set_model",
            Bytes::from(payload),
        )
        .await
        .expect("set_session_model request must succeed");

    // Verify the reply is a valid SetSessionModelResponse.
    let _response: SetSessionModelResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");

    // Verify the model was persisted in the store.
    let state = store.load("sess-model-1").await.unwrap();
    assert_eq!(
        state.model.as_deref(),
        Some("claude-opus-4"),
        "model must be updated in session store"
    );
}

#[tokio::test]
async fn set_session_model_works_for_session_that_does_not_exist() {
    let (_container, nats, js) = start_nats().await;
    let store = start_rpc_server(nats.clone(), js, "acp").await;

    // No pre-existing session — store.load returns Default, which is fine.
    let request = SetSessionModelRequest::new("new-sess", "claude-sonnet-4");
    let payload = serde_json::to_vec(&request).unwrap();
    let reply = nats
        .request(
            "acp.new-sess.agent.session.set_model",
            Bytes::from(payload),
        )
        .await
        .expect("set_session_model must reply even for unknown sessions");

    let _response: SetSessionModelResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");

    // The model is saved under the session ID even though it didn't exist before.
    let state = store.load("new-sess").await.unwrap();
    assert_eq!(state.model.as_deref(), Some("claude-sonnet-4"));
}

#[tokio::test]
async fn set_session_model_bad_payload_does_not_crash_server() {
    let (_container, nats, js) = start_nats().await;
    let _ = start_rpc_server(nats.clone(), js, "acp").await;

    // Send garbage — the server should log a warning and not reply.
    // We verify the server is still alive by sending a valid follow-up request.
    let _ = nats
        .publish(
            "acp.sess-bad.agent.session.set_model",
            Bytes::from_static(b"not json at all"),
        )
        .await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Server should still be up and responsive.
    let request = SetSessionModelRequest::new("sess-alive", "claude-haiku-4-5");
    let payload = serde_json::to_vec(&request).unwrap();
    let reply = nats
        .request(
            "acp.sess-alive.agent.session.set_model",
            Bytes::from(payload),
        )
        .await
        .expect("server must still be alive after bad payload");

    let _response: SetSessionModelResponse =
        serde_json::from_slice(&reply.payload).expect("reply must be valid JSON");
}
