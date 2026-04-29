//! Integration tests for `HashicorpVaultStore` and `vault_admin` with a real
//! HashiCorp Vault server.
//!
//! Requires Docker (testcontainers spins up `hashicorp/vault` in dev mode).
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test hashicorp_vault_e2e

use std::sync::Arc;
use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt, core::ContainerPort, runners::AsyncRunner,
};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, vault_admin, worker,
};
use trogon_vault::{ApiKeyToken, HashicorpVaultConfig, HashicorpVaultStore, VaultAuth, VaultStore};

// ── Constants ─────────────────────────────────────────────────────────────────

const ROOT_TOKEN: &str = "root";
/// KV v2 is mounted at "secret" by default in Vault dev mode (≥ 1.12).
const MOUNT: &str = "secret";
const PREFIX: &str = "test";

// ── Container helpers ─────────────────────────────────────────────────────────

async fn start_vault() -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new("hashicorp/vault", "1.17")
        .with_exposed_port(ContainerPort::Tcp(8200))
        .with_env_var("VAULT_DEV_ROOT_TOKEN_ID", ROOT_TOKEN)
        // Listen on all interfaces so the mapped host port is reachable.
        .with_env_var("VAULT_DEV_LISTEN_ADDRESS", "0.0.0.0:8200")
        .with_startup_timeout(Duration::from_secs(30))
        .start()
        .await
        .expect("Failed to start Vault container — is Docker running?");

    let port = container.get_host_port_ipv4(8200).await.unwrap();
    let addr = format!("http://127.0.0.1:{port}");

    wait_for_vault(&addr).await;

    (container, addr)
}

/// Poll `/v1/sys/health` until Vault returns 200 or timeout elapses.
async fn wait_for_vault(addr: &str) {
    let client = reqwest::Client::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let ready = client
            .get(format!("{addr}/v1/sys/health"))
            .send()
            .await
            .map(|r| r.status().as_u16() == 200)
            .unwrap_or(false);
        if ready {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("Vault did not become ready within 30s");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn start_nats_with_jetstream() -> (ContainerAsync<Nats>, u16) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

async fn nats_client(port: u16) -> async_nats::Client {
    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("Failed to connect to NATS")
}

/// Build a `HashicorpVaultStore` authenticated with the dev root token.
async fn make_store(vault_addr: &str) -> HashicorpVaultStore {
    HashicorpVaultStore::new(HashicorpVaultConfig::new(
        vault_addr,
        MOUNT,
        VaultAuth::Token(ROOT_TOKEN.to_string()),
    ))
    .await
    .expect("Failed to connect to HashiCorp Vault")
}

fn tok(s: &str) -> ApiKeyToken {
    ApiKeyToken::new(s).unwrap()
}

/// Spawn the vault_admin listener backed by a `HashicorpVaultStore`.
async fn spawn_hashicorp_admin(nats: async_nats::Client, vault: Arc<HashicorpVaultStore>) {
    tokio::spawn(async move {
        vault_admin::run(nats, vault, PREFIX, None)
            .await
            .expect("vault admin error");
    });
    // Small delay so subscriptions settle before tests send requests.
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// ── Section 1: HashicorpVaultStore — direct API ───────────────────────────────

/// Storing a token and then resolving it returns the stored plaintext.
#[tokio::test]
async fn hashicorp_vault_store_and_resolve() {
    let (_container, addr) = start_vault().await;
    let store = make_store(&addr).await;

    let token = tok("tok_anthropic_prod_storetest");
    store.store(&token, "sk-ant-real-key").await.unwrap();

    let resolved = store.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-ant-real-key".to_string()));
}

/// Resolving a token that was never stored returns `None`.
#[tokio::test]
async fn hashicorp_vault_resolve_nonexistent_returns_none() {
    let (_container, addr) = start_vault().await;
    let store = make_store(&addr).await;

    let token = tok("tok_openai_prod_noexist001");
    let resolved = store.resolve(&token).await.unwrap();
    assert_eq!(resolved, None);
}

/// After rotating, `resolve` returns the new key, not the original.
#[tokio::test]
async fn hashicorp_vault_rotate_updates_stored_value() {
    let (_container, addr) = start_vault().await;
    let store = make_store(&addr).await;

    let token = tok("tok_gemini_prod_rotatetest");
    store.store(&token, "sk-original-key").await.unwrap();
    store.rotate(&token, "sk-rotated-key").await.unwrap();

    let resolved = store.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-rotated-key".to_string()));
}

/// After revoking, `resolve` returns `None`.
#[tokio::test]
async fn hashicorp_vault_revoke_removes_token() {
    let (_container, addr) = start_vault().await;
    let store = make_store(&addr).await;

    let token = tok("tok_cohere_prod_revoketest");
    store.store(&token, "co-key-to-revoke").await.unwrap();
    assert_eq!(
        store.resolve(&token).await.unwrap(),
        Some("co-key-to-revoke".to_string()),
    );

    store.revoke(&token).await.unwrap();

    assert_eq!(store.resolve(&token).await.unwrap(), None);
}

/// Storing the same token twice overwrites the previous value (idempotent write).
#[tokio::test]
async fn hashicorp_vault_store_twice_overwrites_value() {
    let (_container, addr) = start_vault().await;
    let store = make_store(&addr).await;

    let token = tok("tok_mistral_prod_overwrite1");
    store.store(&token, "first-key").await.unwrap();
    store.store(&token, "second-key").await.unwrap();

    let resolved = store.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("second-key".to_string()));
}

/// Full lifecycle: store → resolve → rotate → resolve → revoke → resolve.
#[tokio::test]
async fn hashicorp_vault_store_rotate_revoke_lifecycle() {
    let (_container, addr) = start_vault().await;
    let store = make_store(&addr).await;

    let token = tok("tok_anthropic_prod_lifecycle1");

    store.store(&token, "initial-key").await.unwrap();
    assert_eq!(
        store.resolve(&token).await.unwrap(),
        Some("initial-key".to_string()),
    );

    store.rotate(&token, "rotated-key").await.unwrap();
    assert_eq!(
        store.resolve(&token).await.unwrap(),
        Some("rotated-key".to_string()),
    );

    store.revoke(&token).await.unwrap();
    assert_eq!(store.resolve(&token).await.unwrap(), None);
}

// ── Section 2: vault_admin over NATS backed by HashicorpVaultStore ────────────

/// Store via NATS and then confirm the key is readable directly from the store.
#[tokio::test]
async fn vault_admin_hashicorp_store_persists_token() {
    let (_vault_container, vault_addr) = start_vault().await;
    let (_nats_container, nats_port) = start_nats().await;

    let store = Arc::new(make_store(&vault_addr).await);
    let nats = nats_client(nats_port).await;
    spawn_hashicorp_admin(nats.clone(), Arc::clone(&store)).await;

    let payload = serde_json::json!({
        "token": "tok_anthropic_prod_natsstoretest",
        "plaintext": "sk-ant-via-nats"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    let resolved = store
        .resolve(&tok("tok_anthropic_prod_natsstoretest"))
        .await
        .unwrap();
    assert_eq!(resolved, Some("sk-ant-via-nats".to_string()));
}

/// Rotate via NATS; the store must reflect the new value.
#[tokio::test]
async fn vault_admin_hashicorp_rotate_updates_value() {
    let (_vault_container, vault_addr) = start_vault().await;
    let (_nats_container, nats_port) = start_nats().await;

    let store = Arc::new(make_store(&vault_addr).await);
    let nats = nats_client(nats_port).await;

    let token = tok("tok_openai_prod_natsrotate01");
    store.store(&token, "sk-original-key").await.unwrap();

    spawn_hashicorp_admin(nats.clone(), Arc::clone(&store)).await;

    let payload = serde_json::json!({
        "token": "tok_openai_prod_natsrotate01",
        "new_plaintext": "sk-rotated-via-nats"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    let resolved = store.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk-rotated-via-nats".to_string()));
}

/// Revoke via NATS; the token must no longer resolve.
#[tokio::test]
async fn vault_admin_hashicorp_revoke_removes_token() {
    let (_vault_container, vault_addr) = start_vault().await;
    let (_nats_container, nats_port) = start_nats().await;

    let store = Arc::new(make_store(&vault_addr).await);
    let nats = nats_client(nats_port).await;

    let token = tok("tok_gemini_prod_natsrevoke01");
    store.store(&token, "gemini-key-to-revoke").await.unwrap();

    spawn_hashicorp_admin(nats.clone(), Arc::clone(&store)).await;

    let payload = serde_json::json!({ "token": "tok_gemini_prod_natsrevoke01" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");

    assert_eq!(store.resolve(&token).await.unwrap(), None);
}

/// Full lifecycle over NATS: store → rotate → revoke, all via vault_admin.
#[tokio::test]
async fn vault_admin_hashicorp_store_rotate_revoke_lifecycle() {
    let (_vault_container, vault_addr) = start_vault().await;
    let (_nats_container, nats_port) = start_nats().await;

    let store = Arc::new(make_store(&vault_addr).await);
    let nats = nats_client(nats_port).await;
    spawn_hashicorp_admin(nats.clone(), Arc::clone(&store)).await;

    let token_str = "tok_anthropic_prod_natslifecycle";

    // store
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&serde_json::json!({
                "token": token_str,
                "plaintext": "initial-key"
            }))
            .unwrap()
            .into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "store: {v}");

    // rotate
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&serde_json::json!({
                "token": token_str,
                "new_plaintext": "rotated-key"
            }))
            .unwrap()
            .into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "rotate: {v}");
    assert_eq!(
        store.resolve(&tok(token_str)).await.unwrap(),
        Some("rotated-key".to_string()),
    );

    // revoke
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&serde_json::json!({ "token": token_str }))
                .unwrap()
                .into(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "revoke: {v}");
    assert_eq!(store.resolve(&tok(token_str)).await.unwrap(), None);
}

/// Invalid token sent to vault_admin must return an error, not panic.
/// Confirms vault_admin with HashicorpVaultStore handles validation errors
/// identically to MemoryVault.
#[tokio::test]
async fn vault_admin_hashicorp_invalid_token_returns_error() {
    let (_vault_container, vault_addr) = start_vault().await;
    let (_nats_container, nats_port) = start_nats().await;

    let store = Arc::new(make_store(&vault_addr).await);
    let nats = nats_client(nats_port).await;
    spawn_hashicorp_admin(nats.clone(), Arc::clone(&store)).await;

    let payload = serde_json::json!({
        "token": "not-a-valid-token",
        "plaintext": "some-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request failed");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "expected ok:false, got: {v}");
    let error = v["error"].as_str().unwrap_or("");
    assert!(
        error.contains("Invalid token"),
        "expected 'Invalid token' in error, got: {error}",
    );
}

/// End-to-end pipeline test: proxy + worker backed by a real HashiCorp Vault.
/// Verifies that an opaque token is resolved to its real API key via Vault and
/// sends a real HTTP request through the proxy+worker with the worker
/// resolving the opaque token via a live HashiCorp Vault instance.
#[tokio::test]
async fn hashicorp_vault_proxy_worker_pipeline_resolves_real_key() {
    let (_vault_container, vault_addr) = start_vault().await;
    let (_nats_container, nats_port) = start_nats_with_jetstream().await;

    // Mock AI provider — must receive the REAL key, not the opaque token.
    let mock_server = httpmock::MockServer::start_async().await;
    let ai_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-from-hashicorp");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_hv","type":"message","content":[]}"#);
        })
        .await;

    // Seed the token directly into HashiCorp Vault.
    let store = Arc::new(make_store(&vault_addr).await);
    let token = tok("tok_anthropic_prod_hvpipeline1");
    store.store(&token, "sk-ant-from-hashicorp").await.unwrap();

    // NATS + JetStream.
    let nats = nats_client(nats_port).await;
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));

    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("Failed to ensure PROXY_REQUESTS stream");

    // Start proxy.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();

    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(mock_server.base_url()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    // Start worker backed by HashiCorp Vault (not MemoryVault).
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_js = jetstream.clone();
    let worker_nats = nats.clone();
    let worker_vault = Arc::clone(&store);
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn(async move {
        worker::run(
            worker_js,
            worker_nats,
            worker_vault,
            http_client,
            "hashicorp-pipeline-worker",
            &worker_stream,
        )
        .await
        .ok();
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send an HTTP request through the proxy using the opaque token.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://127.0.0.1:{proxy_port}/anthropic/v1/messages"
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_hvpipeline1")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("Request to proxy failed");

    assert_eq!(
        resp.status(),
        200,
        "Proxy must return 200 when token resolves via HashiCorp Vault; got {}",
        resp.status()
    );

    // The critical check: the mock only responds to Bearer sk-ant-from-hashicorp.
    // If the worker used an empty MemoryVault or forwarded the opaque token
    // unchanged, the mock would not match and this assertion would fail.
    ai_mock.assert_async().await;
}
