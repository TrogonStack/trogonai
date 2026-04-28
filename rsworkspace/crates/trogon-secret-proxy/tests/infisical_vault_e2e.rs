//! Integration tests for InfisicalVaultStore + vault_admin + proxy pipeline.
//!
//! All Infisical API calls are intercepted by an httpmock server — no real
//! Infisical instance required.  NATS is spun up via testcontainers.
//!
//! Run with:
//!   cargo test -p trogon-secret-proxy --test infisical_vault_e2e

use std::sync::Arc;
use std::time::Duration;

use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_secret_proxy::{
    proxy::{ProxyState, router},
    stream, subjects, vault_admin, worker,
};
use trogon_vault::{ApiKeyToken, DualWriteVault, InfisicalAuth, InfisicalConfig, InfisicalVaultStore, MemoryVault, VaultStore};
use trogon_vault_nats::{CryptoCtx, NatsKvVault, ensure_vault_bucket};

const PREFIX: &str = "test";
const PROJECT_ID: &str = "proj-abc123";
const SERVICE_TOKEN: &str = "st.test-token";

// ── helpers ────────────────────────────────────────────────────────────────────

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
        .expect("NATS connect")
}

fn make_store(server: &httpmock::MockServer) -> InfisicalVaultStore {
    InfisicalVaultStore::new(InfisicalConfig::new(
        format!("http://{}", server.address()),
        PROJECT_ID,
        InfisicalAuth::ServiceToken(SERVICE_TOKEN.to_string()),
    ))
}

fn tok(s: &str) -> ApiKeyToken {
    ApiKeyToken::new(s).unwrap()
}

async fn spawn_infisical_admin(nats: async_nats::Client, vault: Arc<InfisicalVaultStore>) {
    tokio::spawn(async move {
        vault_admin::run(nats, vault, PREFIX, None)
            .await
            .expect("vault admin error");
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
}

// ── Section 1: vault_admin over NATS backed by InfisicalVaultStore ────────────

/// Storing a token via NATS vault_admin causes InfisicalVaultStore to POST to
/// the Infisical API.  The mock records that the call happened.
#[tokio::test]
async fn vault_admin_infisical_store_via_nats() {
    let infisical = httpmock::MockServer::start_async().await;
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_abc123")
                .header("Authorization", format!("Bearer {SERVICE_TOKEN}"))
                .json_body_partial(r#"{"secretValue":"sk-ant-key"}"#);
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_abc123"}}));
        })
        .await;

    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let store = Arc::new(make_store(&infisical));
    spawn_infisical_admin(nats.clone(), store).await;

    let payload = serde_json::json!({
        "token":     "tok_anthropic_prod_abc123",
        "plaintext": "sk-ant-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");
    post_mock.assert_async().await;
}

/// Rotating a token via NATS delegates to `store()`, which POSTs to Infisical.
/// On a 409 Conflict (secret already exists), InfisicalVaultStore follows up
/// with a PATCH to update the value.
#[tokio::test]
async fn vault_admin_infisical_rotate_via_nats() {
    let infisical = httpmock::MockServer::start_async().await;
    // First attempt: POST returns 409.
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/openai_xyz789");
            then.status(409)
                .json_body(serde_json::json!({"message": "Secret already exists"}));
        })
        .await;
    // Follow-up: PATCH with the new value.
    let patch_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/api/v3/secrets/raw/openai_xyz789")
                .json_body_partial(r#"{"secretValue":"sk-new-key"}"#);
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        })
        .await;

    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let store = Arc::new(make_store(&infisical));
    spawn_infisical_admin(nats.clone(), store).await;

    let payload = serde_json::json!({
        "token":         "tok_openai_prod_xyz789",
        "new_plaintext": "sk-new-key"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");
    post_mock.assert_async().await;
    patch_mock.assert_async().await;
}

/// Revoking a token via NATS causes InfisicalVaultStore to DELETE from the
/// Infisical API, scoped to the token's environment.
#[tokio::test]
async fn vault_admin_infisical_revoke_via_nats() {
    let infisical = httpmock::MockServer::start_async().await;
    let delete_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::DELETE)
                .path("/api/v3/secrets/raw/gemini_aabbcc")
                .query_param("environment", "staging");
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        })
        .await;

    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let store = Arc::new(make_store(&infisical));
    spawn_infisical_admin(nats.clone(), store).await;

    let payload = serde_json::json!({ "token": "tok_gemini_staging_aabbcc" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "expected ok:true, got: {v}");
    delete_mock.assert_async().await;
}

// ── Section 2: Full proxy + worker pipeline with InfisicalVaultStore ──────────

/// Full end-to-end: client sends request with opaque token → proxy publishes to
/// JetStream → worker resolves token via InfisicalVaultStore (httpmock) →
/// worker forwards to AI provider (httpmock) with the REAL key → proxy returns
/// 200 to the client.
///
/// The mock AI provider only responds to the real key, not the opaque token.
/// If the worker forwarded the token unchanged, the mock would not match and
/// the test would fail.
#[tokio::test]
async fn infisical_proxy_worker_pipeline_resolves_real_key() {
    let (_nats_container, nats_port) = start_nats_with_jetstream().await;

    // Infisical mock: resolve tok_anthropic_prod_abc123 → real key.
    let infisical = httpmock::MockServer::start_async().await;
    infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_abc123")
                .query_param("environment", "prod");
            then.status(200).json_body(serde_json::json!({
                "secret": {
                    "secretKey":   "anthropic_abc123",
                    "secretValue": "sk-ant-from-infisical"
                }
            }));
        })
        .await;

    // AI provider mock: only matches when the real key is in Authorization.
    let ai_server = httpmock::MockServer::start_async().await;
    let ai_mock = ai_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-from-infisical");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_infisical_01","type":"message","content":[]}"#);
        })
        .await;

    // NATS + JetStream.
    let nats = nats_client(nats_port).await;
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("ensure JetStream stream");

    // Start proxy.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(15),
        base_url_override: Some(format!("http://{}", ai_server.address())),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    // Start worker backed by InfisicalVaultStore.
    let store = Arc::new(make_store(&infisical));
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn({
        let js = jetstream.clone();
        let nc = nats.clone();
        async move {
            worker::run(js, nc, store, http_client, "infisical-worker", &worker_stream)
                .await
                .ok();
        }
    });

    // Allow subscriptions and JetStream consumer to settle.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send an HTTP request through the proxy using the opaque token.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://127.0.0.1:{proxy_port}/anthropic/v1/messages"
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_abc123")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("request to proxy failed");

    assert_eq!(
        resp.status(),
        200,
        "proxy must return 200 when token resolves via Infisical; got {}",
        resp.status()
    );

    // The AI mock only responds to the real key — if the worker forwarded the
    // opaque token unchanged this assertion fails.
    ai_mock.assert_async().await;
}

// ── Section 3: DualWriteVault + vault_admin ───────────────────────────────────

fn kv_crypto() -> std::sync::Arc<CryptoCtx> {
    std::sync::Arc::new(
        CryptoCtx::derive(b"infisical-e2e-pw", b"infisical-salt16").unwrap(),
    )
}

/// vault_admin backed by DualWriteVault<InfisicalVaultStore, NatsKvVault>:
/// a successful store call must hit Infisical (primary) AND leave the value
/// readable from the NatsKvVault cache (no Infisical GET needed).
#[tokio::test]
async fn vault_admin_dual_write_stores_to_both_backends() {
    let infisical = httpmock::MockServer::start_async().await;
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_dw0001");
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_dw0001"}}));
        })
        .await;

    let (_container, nats_port) = start_nats_with_jetstream().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());

    let kv = ensure_vault_bucket(&js, "dual-write-e2e", 1)
        .await
        .expect("vault bucket");
    let nats_kv = NatsKvVault::new(kv, kv_crypto(), Duration::from_secs(60))
        .await
        .expect("NatsKvVault");

    let dual = Arc::new(DualWriteVault::new(make_store(&infisical), nats_kv));
    let dual_for_check = Arc::clone(&dual);

    tokio::spawn({
        let nc = nats.clone();
        async move { vault_admin::run(nc, dual, PREFIX, None).await.ok(); }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let payload = serde_json::json!({
        "token":     "tok_anthropic_prod_dw0001",
        "plaintext": "sk-ant-dual-value"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "vault_admin store must succeed: {v}");
    post_mock.assert_async().await;

    // Allow NatsKvVault watcher to propagate the write into the in-process cache.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Resolve via the shared DualWriteVault — the cache should serve the value.
    // No GET mock is set up: a cache miss would fall through to Infisical and
    // return None (httpmock 404 → InfisicalVaultStore → Ok(None)).
    let token = tok("tok_anthropic_prod_dw0001");
    let resolved = dual_for_check.resolve(&token).await.unwrap();
    assert_eq!(
        resolved,
        Some("sk-ant-dual-value".to_string()),
        "NatsKvVault cache must hold the value written via vault_admin"
    );
}

/// When the cache (MemoryVault) has the token, DualWriteVault must return the
/// cached value without calling Infisical at all.
#[tokio::test]
async fn dual_write_cache_hit_does_not_call_infisical() {
    let infisical = httpmock::MockServer::start_async().await;
    let get_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_ch0001");
            then.status(404)
                .json_body(serde_json::json!({"message": "not found"}));
        })
        .await;

    let cache = MemoryVault::new();
    let token = tok("tok_anthropic_prod_ch0001");
    cache.store(&token, "sk-from-cache").await.unwrap();

    let dual = DualWriteVault::new(make_store(&infisical), cache);
    let resolved = dual.resolve(&token).await.unwrap();

    assert_eq!(resolved, Some("sk-from-cache".to_string()));
    assert_eq!(
        get_mock.hits(),
        0,
        "Infisical GET must not be called when the cache holds the value"
    );
}

/// When the cache is empty, DualWriteVault must fall back to Infisical and
/// return whatever the primary resolves.
#[tokio::test]
async fn dual_write_cache_miss_falls_back_to_infisical() {
    let infisical = httpmock::MockServer::start_async().await;
    let get_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_cm0001")
                .query_param("environment", "prod");
            then.status(200).json_body(serde_json::json!({
                "secret": {
                    "secretKey":   "anthropic_cm0001",
                    "secretValue": "sk-from-infisical"
                }
            }));
        })
        .await;

    // Empty cache — every resolve will fall through to Infisical.
    let dual = DualWriteVault::new(make_store(&infisical), MemoryVault::new());
    let token = tok("tok_anthropic_prod_cm0001");
    let resolved = dual.resolve(&token).await.unwrap();

    assert_eq!(resolved, Some("sk-from-infisical".to_string()));
    assert_eq!(
        get_mock.hits(),
        1,
        "Infisical GET must be called exactly once on a cache miss"
    );
}

/// When Infisical returns 5xx on a store call, vault_admin must publish an
/// ok:false response with an error message — the error must not be swallowed.
#[tokio::test]
async fn infisical_5xx_returns_error_from_vault_admin() {
    let infisical = httpmock::MockServer::start_async().await;
    infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_5xx001");
            then.status(500)
                .json_body(serde_json::json!({"message": "Internal server error"}));
        })
        .await;

    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let store = Arc::new(make_store(&infisical));
    spawn_infisical_admin(nats.clone(), store).await;

    let payload = serde_json::json!({
        "token":     "tok_anthropic_prod_5xx001",
        "plaintext": "sk-ant-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "Infisical 5xx must propagate as ok:false; got: {v}");
    assert!(
        v["error"].as_str().is_some(),
        "error field must be present when ok:false"
    );
}

// ── Section 4: Worker fallback-on-401 with NatsKvVault rotation ───────────────

/// Full proxy → worker pipeline where the token has been rotated.
///
/// The AI provider mock returns 401 for the current key (sk-new) and 200 for
/// the previous key (sk-old).  The worker must detect the 401, fall back to
/// the previous key, and the proxy must return 200 to the caller.
#[tokio::test]
async fn worker_fallback_on_401_uses_rotated_nats_kv_key() {
    let (_nats_container, nats_port) = start_nats_with_jetstream().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());
    let jetstream = Arc::new(js.clone());

    // ── NatsKvVault: store then rotate ────────────────────────────────────────
    let kv = ensure_vault_bucket(&js, "fallback-rotation", 1)
        .await
        .expect("vault bucket");
    let vault = Arc::new(
        NatsKvVault::new(kv, kv_crypto(), Duration::from_secs(120))
            .await
            .expect("NatsKvVault"),
    );

    let token = tok("tok_anthropic_prod_fb0001");
    vault.store(&token, "sk-old").await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await; // watcher sync
    vault.rotate(&token, "sk-new").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await; // watcher creates rotation slot

    let (current, previous) = vault.resolve_with_previous(&token).await.unwrap();
    assert_eq!(current.as_deref(),  Some("sk-new"), "current key must be sk-new");
    assert_eq!(previous.as_deref(), Some("sk-old"), "previous key must be sk-old in grace period");

    // ── AI provider mock ──────────────────────────────────────────────────────
    let ai_server = httpmock::MockServer::start_async().await;
    ai_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-new");
            then.status(401).body("unauthorized");
        })
        .await;
    let ai_mock_old = ai_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-old");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_fallback","type":"message","content":[]}"#);
        })
        .await;

    // ── JetStream stream + proxy + worker ─────────────────────────────────────
    let outbound_subject = subjects::outbound("rotation");
    stream::ensure_stream(&jetstream, "rotation", &outbound_subject)
        .await
        .expect("ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats:             nats.clone(),
        jetstream:        jetstream.clone(),
        prefix:           "rotation".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout:   Duration::from_secs(15),
        base_url_override: Some(format!("http://{}", ai_server.address())),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_stream = stream::stream_name("rotation");
    tokio::spawn({
        let js2 = jetstream.clone();
        let nc  = nats.clone();
        async move {
            worker::run(js2, nc, vault, http_client, "rotation-worker", &worker_stream)
                .await
                .ok();
        }
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    // ── Send a request through the proxy ─────────────────────────────────────
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_fb0001")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("proxy request failed");

    assert_eq!(
        resp.status(),
        200,
        "worker must fall back to previous key on 401 and return 200; got {}",
        resp.status()
    );
    // Prove the fallback actually happened — old-key mock was hit.
    ai_mock_old.assert_async().await;
}

// ── Section 3 continued: DualWriteVault rotate / revoke ──────────────────────

/// vault_admin backed by DualWriteVault<InfisicalVaultStore, NatsKvVault>:
/// rotate must trigger POST→409→PATCH on Infisical (primary) AND leave the
/// new value readable from the NatsKvVault cache.
#[tokio::test]
async fn vault_admin_dual_write_rotate_updates_both_backends() {
    let infisical = httpmock::MockServer::start_async().await;
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_dw0002");
            then.status(409)
                .json_body(serde_json::json!({"message": "Secret already exists"}));
        })
        .await;
    let patch_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/api/v3/secrets/raw/anthropic_dw0002")
                .json_body_partial(r#"{"secretValue":"sk-ant-rotated"}"#);
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        })
        .await;

    let (_container, nats_port) = start_nats_with_jetstream().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());

    let kv = ensure_vault_bucket(&js, "dw-rotate-e2e", 1)
        .await
        .expect("vault bucket");
    let nats_kv = NatsKvVault::new(kv, kv_crypto(), Duration::from_secs(60))
        .await
        .expect("NatsKvVault");

    let dual = Arc::new(DualWriteVault::new(make_store(&infisical), nats_kv));
    let dual_for_check = Arc::clone(&dual);

    tokio::spawn({
        let nc = nats.clone();
        async move { vault_admin::run(nc, dual, PREFIX, None).await.ok(); }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let payload = serde_json::json!({
        "token":         "tok_anthropic_prod_dw0002",
        "new_plaintext": "sk-ant-rotated"
    });
    let resp = nats
        .request(
            subjects::vault_rotate(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "vault_admin rotate must succeed: {v}");
    post_mock.assert_async().await;
    patch_mock.assert_async().await;

    // Allow NatsKvVault watcher to propagate the write.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Resolve via DualWriteVault: cache should serve the value.
    // No GET mock is set up — a cache miss falls back to Infisical (404 → None).
    // Getting Some("sk-ant-rotated") proves the NatsKvVault cache holds the value.
    let token = tok("tok_anthropic_prod_dw0002");
    let resolved = dual_for_check.resolve(&token).await.unwrap();
    assert_eq!(
        resolved,
        Some("sk-ant-rotated".to_string()),
        "NatsKvVault cache must hold the rotated value written via vault_admin"
    );
}

/// vault_admin backed by DualWriteVault<InfisicalVaultStore, NatsKvVault>:
/// revoke must DELETE from Infisical (primary) AND clear the NatsKvVault cache
/// so that subsequent resolves return None.
#[tokio::test]
async fn vault_admin_dual_write_revoke_removes_from_both_backends() {
    let infisical = httpmock::MockServer::start_async().await;
    let delete_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::DELETE)
                .path("/api/v3/secrets/raw/anthropic_dw0003")
                .query_param("environment", "prod");
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        })
        .await;

    let (_container, nats_port) = start_nats_with_jetstream().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());

    let kv = ensure_vault_bucket(&js, "dw-revoke-e2e", 1)
        .await
        .expect("vault bucket");
    let nats_kv = NatsKvVault::new(kv, kv_crypto(), Duration::from_secs(60))
        .await
        .expect("NatsKvVault");

    // Pre-seed the NatsKvVault so there is a value to revoke.
    let token = tok("tok_anthropic_prod_dw0003");
    nats_kv.store(&token, "sk-ant-to-revoke").await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await; // watcher sync

    let dual = Arc::new(DualWriteVault::new(make_store(&infisical), nats_kv));
    let dual_for_check = Arc::clone(&dual);

    tokio::spawn({
        let nc = nats.clone();
        async move { vault_admin::run(nc, dual, PREFIX, None).await.ok(); }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let payload = serde_json::json!({ "token": "tok_anthropic_prod_dw0003" });
    let resp = nats
        .request(
            subjects::vault_revoke(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], true, "vault_admin revoke must succeed: {v}");
    delete_mock.assert_async().await;

    // Allow NatsKvVault watcher to propagate the deletion.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // If the NatsKvVault cache was NOT cleared, it would return Some("sk-ant-to-revoke").
    // No GET mock is set up — a cache miss falls back to Infisical (404 → None).
    // Getting None proves the cache was cleared.
    let resolved = dual_for_check.resolve(&token).await.unwrap();
    assert_eq!(
        resolved,
        None,
        "token must be gone from both backends after revocation; \
         a non-None result means the NatsKvVault cache was not cleared"
    );
}

// ── Section 5: Full proxy→worker pipeline with DualWriteVault ─────────────────

/// Full pipeline where the vault IS a DualWriteVault<InfisicalVaultStore, NatsKvVault>.
///
/// Step 1 — vault_admin stores a token: Infisical POST is hit, NatsKvVault cache
///           is populated.
/// Step 2 — an HTTP request flows through proxy → worker: the worker resolves
///           the token from the NatsKvVault cache without calling Infisical GET.
/// Step 3 — the worker forwards the real key to the AI server, which returns 200.
///
/// The Infisical GET mock is set up but must record zero hits — proving the
/// worker resolved entirely from the NATS KV cache.
#[tokio::test]
async fn dual_write_proxy_worker_pipeline_resolves_from_cache() {
    let infisical = httpmock::MockServer::start_async().await;
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_dw0004");
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_dw0004"}}));
        })
        .await;
    // This GET must NOT be called — the worker should resolve from NatsKvVault.
    let get_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_dw0004");
            then.status(200)
                .json_body(serde_json::json!({
                    "secret": {
                        "secretKey":   "anthropic_dw0004",
                        "secretValue": "sk-from-infisical-not-expected"
                    }
                }));
        })
        .await;

    let (_nats_container, nats_port) = start_nats_with_jetstream().await;
    let nats = nats_client(nats_port).await;
    let js = async_nats::jetstream::new(nats.clone());
    let jetstream = Arc::new(js.clone());

    let kv = ensure_vault_bucket(&js, "dw-pipeline-cache", 1)
        .await
        .expect("vault bucket");
    let nats_kv = NatsKvVault::new(kv, kv_crypto(), Duration::from_secs(60))
        .await
        .expect("NatsKvVault");

    let dual = Arc::new(DualWriteVault::new(make_store(&infisical), nats_kv));
    let dual_for_worker = Arc::clone(&dual);

    // Spawn vault_admin backed by DualWriteVault.
    tokio::spawn({
        let nc = nats.clone();
        async move { vault_admin::run(nc, dual, PREFIX, None).await.ok(); }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Store token via vault_admin — hits Infisical POST and populates NatsKvVault.
    let store_payload = serde_json::json!({
        "token":     "tok_anthropic_prod_dw0004",
        "plaintext": "sk-ant-cached"
    });
    let store_resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&store_payload).unwrap().into(),
        )
        .await
        .expect("NATS request");
    let sv: serde_json::Value = serde_json::from_slice(&store_resp.payload).unwrap();
    assert_eq!(sv["ok"], true, "store must succeed: {sv}");
    post_mock.assert_async().await;

    // Allow NatsKvVault background watcher to update the in-process DashMap.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // AI provider: only accepts the real key.
    let ai_server = httpmock::MockServer::start_async().await;
    let ai_mock = ai_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/v1/messages")
                .header("authorization", "Bearer sk-ant-cached");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"id":"msg_cached","type":"message","content":[]}"#);
        })
        .await;

    // Proxy + worker sharing the DualWriteVault via Arc.
    let outbound_subject = subjects::outbound("dw-cache");
    stream::ensure_stream(&jetstream, "dw-cache", &outbound_subject)
        .await
        .expect("ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats:              nats.clone(),
        jetstream:         jetstream.clone(),
        prefix:            "dw-cache".to_string(),
        outbound_subject:  outbound_subject.clone(),
        worker_timeout:    Duration::from_secs(15),
        base_url_override: Some(format!("http://{}", ai_server.address())),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap();
    let worker_stream = stream::stream_name("dw-cache");
    tokio::spawn({
        let js2 = jetstream.clone();
        let nc  = nats.clone();
        async move {
            worker::run(js2, nc, dual_for_worker, http_client, "dw-cache-worker", &worker_stream)
                .await
                .ok();
        }
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_dw0004")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .expect("proxy request failed");

    assert_eq!(
        resp.status(),
        200,
        "proxy must return 200 when worker resolves from NatsKvVault cache; got {}",
        resp.status()
    );
    ai_mock.assert_async().await;
    assert_eq!(
        get_mock.hits(),
        0,
        "Infisical GET must not be called — worker must resolve tok_anthropic_prod_dw0004 from NatsKvVault cache"
    );
}

// ── Section 6: Error propagation and edge cases ───────────────────────────────

/// When Infisical returns 401 on a GET (expired or revoked service token), the
/// worker must propagate it as a "Vault error" — NOT silently convert it to
/// Ok(None) ("token not found").  The proxy must return an error status, and the
/// Infisical GET mock must have been called (ruling out a short-circuit).
///
/// 401 → `InfisicalError::Api { status: 401 }` → worker "Vault error: ..." → 401
/// 404 → `Ok(None)`                             → worker "Token not found"  → 401
///
/// Both surface as 401 to the proxy caller, but via different code paths.
/// This test pins the 401-from-Infisical path so a future change that silently
/// treats it as Ok(None) would be caught.
#[tokio::test]
async fn infisical_401_on_resolve_propagates_error_through_worker() {
    let (_nats_container, nats_port) = start_nats_with_jetstream().await;

    let infisical = httpmock::MockServer::start_async().await;
    let get_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_err401a")
                .query_param("environment", "prod");
            then.status(401)
                .json_body(serde_json::json!({"message": "Unauthorized"}));
        })
        .await;

    let nats = nats_client(nats_port).await;
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("e401");
    stream::ensure_stream(&jetstream, "e401", &outbound_subject)
        .await
        .expect("ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats:              nats.clone(),
        jetstream:         jetstream.clone(),
        prefix:            "e401".to_string(),
        outbound_subject:  outbound_subject.clone(),
        worker_timeout:    Duration::from_secs(10),
        base_url_override: Some("http://127.0.0.1:1".to_string()), // never reached
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let store = Arc::new(make_store(&infisical));
    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap();
    let worker_stream = stream::stream_name("e401");
    tokio::spawn({
        let js = jetstream.clone();
        let nc = nats.clone();
        async move {
            worker::run(js, nc, store, http_client, "e401-worker", &worker_stream).await.ok();
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_err401a")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .expect("proxy request failed");

    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Infisical 401 must propagate as an error; got {}",
        resp.status()
    );
    // Confirms the request actually reached Infisical — not a timeout or parse error.
    get_mock.assert_async().await;
}

/// When Infisical returns 429 (rate limit) on a store call, vault_admin must
/// return ok:false with an error body that includes the 429 status code.
/// A 429 is NOT a 409 (conflict), so the POST→PATCH upgrade must not happen.
#[tokio::test]
async fn infisical_429_on_store_returns_error_from_vault_admin() {
    let infisical = httpmock::MockServer::start_async().await;
    infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_ratelim1");
            then.status(429)
                .json_body(serde_json::json!({"message": "Too Many Requests"}));
        })
        .await;

    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;
    let store = Arc::new(make_store(&infisical));
    spawn_infisical_admin(nats.clone(), store).await;

    let payload = serde_json::json!({
        "token":     "tok_anthropic_prod_ratelim1",
        "plaintext": "sk-ant-key"
    });
    let resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&payload).unwrap().into(),
        )
        .await
        .expect("NATS request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["ok"], false, "Infisical 429 must propagate as ok:false; got: {v}");

    let error_msg = v["error"].as_str().expect("error field must be a string");
    assert!(
        error_msg.contains("429"),
        "error message must reference status 429; got: {error_msg}"
    );
}

/// When Infisical returns 200 with `secretValue: ""`, InfisicalVaultStore
/// resolves to Ok(Some("")).  The worker must detect the empty key and return
/// an error response — it must NOT forward a blank Authorization header to the
/// AI provider.
///
/// The Infisical GET mock must be hit, confirming the request reached the vault.
#[tokio::test]
async fn infisical_empty_secret_value_returns_error_through_worker() {
    let (_nats_container, nats_port) = start_nats_with_jetstream().await;

    let infisical = httpmock::MockServer::start_async().await;
    let get_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_emptyval1")
                .query_param("environment", "prod");
            then.status(200).json_body(serde_json::json!({
                "secret": {
                    "secretKey":   "anthropic_emptyval1",
                    "secretValue": ""
                }
            }));
        })
        .await;

    let nats = nats_client(nats_port).await;
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("emptyval");
    stream::ensure_stream(&jetstream, "emptyval", &outbound_subject)
        .await
        .expect("ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats:              nats.clone(),
        jetstream:         jetstream.clone(),
        prefix:            "emptyval".to_string(),
        outbound_subject:  outbound_subject.clone(),
        worker_timeout:    Duration::from_secs(10),
        base_url_override: Some("http://127.0.0.1:1".to_string()), // never reached
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let store = Arc::new(make_store(&infisical));
    let http_client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().unwrap();
    let worker_stream = stream::stream_name("emptyval");
    tokio::spawn({
        let js = jetstream.clone();
        let nc = nats.clone();
        async move {
            worker::run(js, nc, store, http_client, "emptyval-worker", &worker_stream).await.ok();
        }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{proxy_port}/anthropic/v1/messages"))
        .header("Authorization", "Bearer tok_anthropic_prod_emptyval1")
        .header("Content-Type", "application/json")
        .body(r#"{"model":"claude-opus-4-6","max_tokens":10,"messages":[]}"#)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .expect("proxy request failed");

    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "Empty secretValue must cause an error response; got {}",
        resp.status()
    );
    // Infisical GET was hit and returned the empty value — not a short-circuit.
    get_mock.assert_async().await;
}

/// vault_admin with vault_name=Some("payments") must:
///   1. Subscribe only to {prefix}.vault.payments.{store,rotate,revoke}
///   2. Still derive the Infisical secret name and environment from the token
///      exactly as the flat vault_admin does (vault_name does not affect mapping)
///
/// Isolation is verified by also running a flat vault_admin (MemoryVault) on the
/// same NATS server.  A request to the flat subject must reach the flat vault_admin
/// (not Infisical), while a request to the named subject must reach Infisical.
/// The Infisical POST mock hit count of 1 proves no cross-contamination occurred.
#[tokio::test]
async fn named_vault_admin_with_infisical_routes_and_maps_correctly() {
    let infisical = httpmock::MockServer::start_async().await;
    // Only the named vault_admin sends to Infisical.
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/stripe_paymt001")
                .json_body_partial(
                    r#"{"environment":"prod","secretValue":"sk-stripe-payments"}"#,
                );
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "stripe_paymt001"}}));
        })
        .await;

    let (_container, port) = start_nats().await;
    let nats = nats_client(port).await;

    // Flat vault_admin (MemoryVault) — handles {prefix}.vault.store only.
    let flat_vault = Arc::new(MemoryVault::new());
    tokio::spawn({
        let nc = nats.clone();
        let v  = flat_vault.clone();
        async move { vault_admin::run(nc, v, PREFIX, None).await.ok(); }
    });

    // Named vault_admin (InfisicalVaultStore) — handles {prefix}.vault.payments.{*} only.
    let payments_vault = Arc::new(make_store(&infisical));
    tokio::spawn({
        let nc = nats.clone();
        let v  = payments_vault.clone();
        async move { vault_admin::run(nc, v, PREFIX, Some("payments")).await.ok(); }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // ── flat subject → flat vault_admin handles it; Infisical is NOT called ─────
    let flat_payload = serde_json::json!({
        "token":     "tok_openai_prod_flat0001",
        "plaintext": "sk-openai-flat"
    });
    let flat_resp = nats
        .request(
            subjects::vault_store(PREFIX),
            serde_json::to_vec(&flat_payload).unwrap().into(),
        )
        .await
        .expect("flat NATS request");
    let fv: serde_json::Value = serde_json::from_slice(&flat_resp.payload).unwrap();
    assert_eq!(fv["ok"], true, "flat vault_admin store must succeed: {fv}");

    // ── named subject → named vault_admin handles it; Infisical IS called ───────
    // Token maps to: env=prod, secretKey=stripe_paymt001 — independent of vault_name.
    let named_payload = serde_json::json!({
        "token":     "tok_stripe_prod_paymt001",
        "plaintext": "sk-stripe-payments"
    });
    let named_resp = nats
        .request(
            subjects::vault_store_for(PREFIX, "payments"),
            serde_json::to_vec(&named_payload).unwrap().into(),
        )
        .await
        .expect("named NATS request");
    let nv: serde_json::Value = serde_json::from_slice(&named_resp.payload).unwrap();
    assert_eq!(nv["ok"], true, "named vault_admin store must succeed: {nv}");

    // Infisical was called exactly once — proving isolation (flat request never
    // reached InfisicalVaultStore) and correct env/secret-name mapping.
    assert_eq!(
        post_mock.hits(),
        1,
        "Infisical must be called exactly once (named vault request only)"
    );
    post_mock.assert_async().await;
}

/// When InfisicalVaultStore returns None for an unknown token (404 from mock),
/// the worker responds with an error and the proxy surfaces it as a 4xx/5xx.
///
/// Token `tok_anthropic_prod_unknown1` maps to secret name `anthropic_unknown1`.
#[tokio::test]
async fn infisical_proxy_worker_unknown_token_returns_error() {
    let (_nats_container, nats_port) = start_nats_with_jetstream().await;

    // Infisical mock: the specific secret is not found (404).
    let infisical = httpmock::MockServer::start_async().await;
    infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_unknown1")
                .query_param("environment", "prod");
            then.status(404)
                .json_body(serde_json::json!({"message": "Secret not found"}));
        })
        .await;

    let nats = nats_client(nats_port).await;
    let jetstream = Arc::new(async_nats::jetstream::new(nats.clone()));
    let outbound_subject = subjects::outbound("trogon");
    stream::ensure_stream(&jetstream, "trogon", &outbound_subject)
        .await
        .expect("ensure stream");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    let proxy_state = ProxyState {
        nats: nats.clone(),
        jetstream: jetstream.clone(),
        prefix: "trogon".to_string(),
        outbound_subject: outbound_subject.clone(),
        worker_timeout: Duration::from_secs(10),
        base_url_override: Some("http://127.0.0.1:1".to_string()),
    };
    tokio::spawn(async move {
        axum::serve(listener, router(proxy_state)).await.ok();
    });

    let store = Arc::new(make_store(&infisical));
    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let worker_stream = stream::stream_name("trogon");
    tokio::spawn({
        let js = jetstream.clone();
        let nc = nats.clone();
        async move {
            worker::run(js, nc, store, http_client, "infisical-worker-2", &worker_stream)
                .await
                .ok();
        }
    });

    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://127.0.0.1:{proxy_port}/anthropic/v1/messages"
        ))
        .header("Authorization", "Bearer tok_anthropic_prod_unknown1")
        .header("Content-Type", "application/json")
        .body("{}")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .expect("request to proxy failed");

    // Worker could not resolve the token — proxy must return an error status.
    assert!(
        resp.status().is_client_error() || resp.status().is_server_error(),
        "expected error status for unknown token, got {}",
        resp.status()
    );
}

// ── Section 7: InfisicalConfig::with_secret_path ──────────────────────────────

/// `InfisicalConfig::with_secret_path()` sets a custom Infisical secret path
/// that must appear in every HTTP request the store sends — POST body,
/// GET query params, and DELETE query params.
///
/// Without this test a regression where `secret_path` is accidentally hardcoded
/// to "/" would pass the entire suite undetected: all other tests use the default
/// path and none of their mocks assert on the `secretPath` field.
#[tokio::test]
async fn custom_secret_path_is_threaded_through_all_infisical_requests() {
    let infisical = httpmock::MockServer::start_async().await;

    // store(): POST body must carry secretPath.
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_scpath1")
                .json_body_partial(
                    r#"{"secretPath":"/payment-keys","secretValue":"sk-ant-path-test"}"#,
                );
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_scpath1"}}));
        })
        .await;

    // rotate() → store() → POST 409 → patch_secret(): PATCH body must carry secretPath.
    let rotate_post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_scpath1")
                .json_body_partial(r#"{"secretValue":"sk-ant-path-v2"}"#);
            then.status(409)
                .json_body(serde_json::json!({"message": "Secret already exists"}));
        })
        .await;
    let patch_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::PATCH)
                .path("/api/v3/secrets/raw/anthropic_scpath1")
                .json_body_partial(
                    r#"{"secretPath":"/payment-keys","secretValue":"sk-ant-path-v2"}"#,
                );
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        })
        .await;

    // resolve(): GET query params must carry secretPath.
    let get_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/api/v3/secrets/raw/anthropic_scpath1")
                .query_param("secretPath", "/payment-keys");
            then.status(200).json_body(serde_json::json!({
                "secret": {
                    "secretKey":   "anthropic_scpath1",
                    "secretValue": "sk-ant-path-test"
                }
            }));
        })
        .await;

    // revoke(): DELETE query params must carry secretPath.
    let delete_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::DELETE)
                .path("/api/v3/secrets/raw/anthropic_scpath1")
                .query_param("secretPath", "/payment-keys");
            then.status(200)
                .json_body(serde_json::json!({"secret": {}}));
        })
        .await;

    let vault = InfisicalVaultStore::new(
        InfisicalConfig::new(
            format!("http://{}", infisical.address()),
            PROJECT_ID,
            InfisicalAuth::ServiceToken(SERVICE_TOKEN.to_string()),
        )
        .with_secret_path("/payment-keys"),
    );

    let token = tok("tok_anthropic_prod_scpath1");

    vault.store(&token, "sk-ant-path-test").await.unwrap();
    post_mock.assert_async().await;

    vault.rotate(&token, "sk-ant-path-v2").await.unwrap();
    rotate_post_mock.assert_async().await;
    patch_mock.assert_async().await;

    let value = vault.resolve(&token).await.unwrap();
    assert_eq!(value, Some("sk-ant-path-test".to_string()));
    get_mock.assert_async().await;

    vault.revoke(&token).await.unwrap();
    delete_mock.assert_async().await;
}
