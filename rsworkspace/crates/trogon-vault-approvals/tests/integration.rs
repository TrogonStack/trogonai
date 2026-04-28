//! Integration tests for ApprovalService — require a live NATS server.
//!
//! Run with:
//! ```sh
//! cargo test -p trogon-vault-approvals
//! ```
//! testcontainers spins up NATS automatically.

use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream;
use futures_util::StreamExt as _;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_vault::{ApiKeyToken, DualWriteVault, InfisicalAuth, InfisicalConfig, InfisicalVaultStore, MemoryVault, VaultStore};
use trogon_vault_approvals::{
    ApprovalService, JetStreamStatePublisher, NoopNotifier,
    ensure_proposals_stream,
    subjects,
    state_update_subject,
};
use trogon_vault_nats::{CryptoCtx, NatsKvVault, ensure_vault_bucket};

// ── Helpers ───────────────────────────────────────────────────────────────────

async fn start_nats() -> (jetstream::Context, async_nats::Client, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats.clone());
    (js, nats, container)
}

/// Spawn an ApprovalService in the background with MemoryVault + NoopNotifier.
/// Returns the vault so tests can inspect its state.
async fn spawn_service(
    js:         jetstream::Context,
    nats:       async_nats::Client,
    vault_name: &str,
) -> Arc<MemoryVault> {
    ensure_proposals_stream(&js).await.expect("proposals stream");

    let vault     = Arc::new(MemoryVault::new());
    let notifier  = Arc::new(NoopNotifier);
    let publisher = Arc::new(JetStreamStatePublisher::new(js.clone()));
    let svc       = ApprovalService::new(
        Arc::clone(&vault),
        notifier,
        publisher,
        vault_name,
    );

    tokio::spawn(async move { svc.run(js, nats).await.ok(); });
    tokio::time::sleep(Duration::from_millis(100)).await;
    vault
}

// ── Helper: publish approve/reject via core NATS (not JetStream) ─────────────
// Approve and reject messages carry plaintext API keys — they must NOT be stored
// in JetStream. Tests publish them as plain core-NATS messages to match production.

async fn publish_approve(nats: &async_nats::Client, vault_name: &str, payload: serde_json::Value) {
    nats.publish(
        subjects::approve(vault_name),
        serde_json::to_vec(&payload).unwrap().into(),
    )
    .await
    .expect("publish approve");
}

async fn publish_reject(nats: &async_nats::Client, vault_name: &str, payload: serde_json::Value) {
    nats.publish(
        subjects::reject(vault_name),
        serde_json::to_vec(&payload).unwrap().into(),
    )
    .await
    .expect("publish reject");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_proposal_is_stored_in_memory() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "default").await;

    js.publish(
        subjects::create("default"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_create001",
            "credential_key": "tok_anthropic_prod_abc123",
            "service":        "api.anthropic.com",
            "message":        "need claude access"
        }))
        .unwrap()
        .into(),
    )
    .await
    .expect("publish")
    .await
    .expect("ack");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let status_resp = nats
        .request(
            subjects::status("default", "prop_create001"),
            bytes::Bytes::new(),
        )
        .await
        .expect("status request");

    let v: serde_json::Value = serde_json::from_slice(&status_resp.payload).unwrap();
    assert_eq!(v["status"], "pending", "newly created proposal must be pending: {v}");
    assert_eq!(v["proposal_id"], "prop_create001");
}

#[tokio::test]
async fn approve_proposal_writes_to_vault_and_updates_status() {
    let (js, nats, _c) = start_nats().await;
    let vault = spawn_service(js.clone(), nats.clone(), "prod").await;

    js.publish(
        subjects::create("prod"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_approve001",
            "credential_key": "tok_stripe_prod_abc123",
            "service":        "api.stripe.com",
            "message":        "stripe for payments"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_approve(&nats, "prod", serde_json::json!({
        "proposal_id": "prop_approve001",
        "approved_by": "mario",
        "plaintext":   "sk_live_supersecret"
    })).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let token = ApiKeyToken::new("tok_stripe_prod_abc123").unwrap();
    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk_live_supersecret".to_string()), "vault must hold plaintext after approval");

    let resp = nats
        .request(subjects::status("prod", "prop_approve001"), bytes::Bytes::new())
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["status"], "approved");
    assert_eq!(v["approved_by"], "mario");
}

#[tokio::test]
async fn reject_proposal_does_not_write_vault_and_updates_status() {
    let (js, nats, _c) = start_nats().await;
    let vault = spawn_service(js.clone(), nats.clone(), "staging").await;

    js.publish(
        subjects::create("staging"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_reject001",
            "credential_key": "tok_openai_staging_xyz789",
            "service":        "api.openai.com",
            "message":        "GPT-4 for research"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_reject(&nats, "staging", serde_json::json!({
        "proposal_id": "prop_reject001",
        "rejected_by": "luigi",
        "reason":      "budget exceeded"
    })).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let token = ApiKeyToken::new("tok_openai_staging_xyz789").unwrap();
    assert_eq!(vault.resolve(&token).await.unwrap(), None, "rejected proposal must not write vault");

    let resp = nats
        .request(subjects::status("staging", "prop_reject001"), bytes::Bytes::new())
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["status"], "rejected");
    assert_eq!(v["rejected_by"], "luigi");
    assert_eq!(v["reason"], "budget exceeded");
}

#[tokio::test]
async fn status_returns_not_found_for_unknown_proposal() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "default").await;

    let resp = nats
        .request(
            subjects::status("default", "prop_does_not_exist"),
            bytes::Bytes::new(),
        )
        .await
        .expect("status request");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["status"], "not_found");
}

#[tokio::test]
async fn approve_unknown_proposal_is_ignored_and_service_continues() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "default").await;

    publish_approve(&nats, "default", serde_json::json!({
        "proposal_id": "prop_ghost",
        "approved_by": "ghost",
        "plaintext":   "sk_ghost"
    })).await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    let resp = nats
        .request(subjects::status("default", "prop_ghost"), bytes::Bytes::new())
        .await
        .expect("service must still respond after orphaned approve");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["status"], "not_found");
}

#[tokio::test]
async fn second_approve_for_already_approved_proposal_is_ignored() {
    let (js, nats, _c) = start_nats().await;
    let vault = spawn_service(js.clone(), nats.clone(), "prod").await;

    js.publish(
        subjects::create("prod"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_double001",
            "credential_key": "tok_anthropic_prod_double1",
            "service":        "api.anthropic.com",
            "message":        "double approve test"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_approve(&nats, "prod", serde_json::json!({
        "proposal_id": "prop_double001",
        "approved_by": "mario",
        "plaintext":   "sk-first"
    })).await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_approve(&nats, "prod", serde_json::json!({
        "proposal_id": "prop_double001",
        "approved_by": "luigi",
        "plaintext":   "sk-second"
    })).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let token = ApiKeyToken::new("tok_anthropic_prod_double1").unwrap();
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        Some("sk-first".to_string()),
        "second approve must not overwrite first"
    );
}

#[tokio::test]
async fn named_vaults_are_isolated() {
    let (js, nats, _c) = start_nats().await;
    let vault_prod    = spawn_service(js.clone(), nats.clone(), "prod").await;
    let vault_staging = spawn_service(js.clone(), nats.clone(), "staging").await;

    let token = ApiKeyToken::new("tok_anthropic_prod_abc123").unwrap();

    js.publish(
        subjects::create("prod"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_iso001",
            "credential_key": "tok_anthropic_prod_abc123",
            "service":        "api.anthropic.com",
            "message":        "prod access"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_approve(&nats, "prod", serde_json::json!({
        "proposal_id": "prop_iso001",
        "approved_by": "mario",
        "plaintext":   "sk-prod-key"
    })).await;

    tokio::time::sleep(Duration::from_millis(200)).await;

    assert_eq!(
        vault_prod.resolve(&token).await.unwrap(),
        Some("sk-prod-key".to_string())
    );
    assert_eq!(vault_staging.resolve(&token).await.unwrap(), None);
}

#[tokio::test]
async fn approve_publishes_state_update_to_stream() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "prod").await;

    let mut state_sub = nats
        .subscribe(state_update_subject("prod", ">"))
        .await
        .expect("state sub");

    js.publish(
        subjects::create("prod"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_state001",
            "credential_key": "tok_stripe_prod_state1",
            "service":        "api.stripe.com",
            "message":        "state update test"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_approve(&nats, "prod", serde_json::json!({
        "proposal_id": "prop_state001",
        "approved_by": "mario",
        "plaintext":   "sk_live_state_test"
    })).await;

    let msg = tokio::time::timeout(Duration::from_secs(2), state_sub.next())
        .await
        .expect("timeout waiting for state update")
        .expect("state sub closed");

    let v: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(v["state"], "approved", "state update must say approved: {v}");
    assert_eq!(v["by"], "mario");
    assert_eq!(v["proposal_id"], "prop_state001");
    assert_eq!(v["vault"], "prod");
}

#[tokio::test]
async fn reject_publishes_state_update_to_stream() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "staging").await;

    let mut state_sub = nats
        .subscribe(state_update_subject("staging", ">"))
        .await
        .expect("state sub");

    js.publish(
        subjects::create("staging"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_rstate001",
            "credential_key": "tok_openai_staging_rstate1",
            "service":        "api.openai.com",
            "message":        "reject state test"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    publish_reject(&nats, "staging", serde_json::json!({
        "proposal_id": "prop_rstate001",
        "rejected_by": "luigi",
        "reason":      "policy violation"
    })).await;

    let msg = tokio::time::timeout(Duration::from_secs(2), state_sub.next())
        .await
        .expect("timeout waiting for state update")
        .expect("state sub closed");

    let v: serde_json::Value = serde_json::from_slice(&msg.payload).unwrap();
    assert_eq!(v["state"], "rejected");
    assert_eq!(v["by"], "luigi");
    assert_eq!(v["reason"], "policy violation");
}

// ── NatsKvVault backend ───────────────────────────────────────────────────────

fn crypto() -> Arc<CryptoCtx> {
    Arc::new(CryptoCtx::derive(b"approval-test-pw", b"approval-salt-16").unwrap())
}

async fn make_nats_kv_vault(js: &jetstream::Context, name: &str) -> NatsKvVault {
    let kv = ensure_vault_bucket(js, name, 1).await.expect("bucket");
    NatsKvVault::new(kv, crypto(), Duration::from_secs(30))
        .await
        .expect("vault")
}

/// Approving a proposal causes the plaintext to be written to a NatsKvVault —
/// verifying that the approval workflow integrates with the encrypted KV backend.
#[tokio::test]
async fn approve_proposal_writes_to_nats_kv_vault() {
    let (js, nats, _c) = start_nats().await;

    ensure_proposals_stream(&js).await.expect("proposals stream");

    let vault     = Arc::new(make_nats_kv_vault(&js, "approvals-kv-test").await);
    let notifier  = Arc::new(NoopNotifier);
    let publisher = Arc::new(JetStreamStatePublisher::new(js.clone()));
    let svc = ApprovalService::new(Arc::clone(&vault), notifier, publisher, "default");
    let (js_svc, nats_svc) = (js.clone(), nats.clone());
    tokio::spawn(async move { svc.run(js_svc, nats_svc).await.ok(); });
    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish(
        subjects::create("default"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_kv001",
            "credential_key": "tok_anthropic_prod_kv0001",
            "service":        "api.anthropic.com",
            "message":        "need access"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    nats.publish(
        subjects::approve("default"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_kv001",
            "approved_by": "mario",
            "plaintext":   "sk-ant-from-kv-approval"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(300)).await;

    let token = ApiKeyToken::new("tok_anthropic_prod_kv0001").unwrap();
    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(
        resolved,
        Some("sk-ant-from-kv-approval".to_string()),
        "approved plaintext must be readable from NatsKvVault"
    );
}

/// Rejecting a proposal must NOT write anything to NatsKvVault.
#[tokio::test]
async fn reject_proposal_does_not_write_to_nats_kv_vault() {
    let (js, nats, _c) = start_nats().await;

    ensure_proposals_stream(&js).await.expect("proposals stream");

    let vault     = Arc::new(make_nats_kv_vault(&js, "approvals-kv-rej-test").await);
    let notifier  = Arc::new(NoopNotifier);
    let publisher = Arc::new(JetStreamStatePublisher::new(js.clone()));
    let svc = ApprovalService::new(Arc::clone(&vault), notifier, publisher, "default");
    let (js_svc, nats_svc) = (js.clone(), nats.clone());
    tokio::spawn(async move { svc.run(js_svc, nats_svc).await.ok(); });
    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish(
        subjects::create("default"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_kv_rej001",
            "credential_key": "tok_openai_staging_kv0001",
            "service":        "api.openai.com",
            "message":        "need access"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    nats.publish(
        subjects::reject("default"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_kv_rej001",
            "rejected_by": "luigi",
            "reason":      "not needed"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let token = ApiKeyToken::new("tok_openai_staging_kv0001").unwrap();
    assert_eq!(
        vault.resolve(&token).await.unwrap(),
        None,
        "rejected proposal must not write to NatsKvVault"
    );
}

// ── InfisicalVaultStore backend ───────────────────────────────────────────────

fn make_infisical_store(server: &httpmock::MockServer) -> InfisicalVaultStore {
    InfisicalVaultStore::new(InfisicalConfig::new(
        format!("http://{}", server.address()),
        "proj-test123",
        InfisicalAuth::ServiceToken("st.test".to_string()),
    ))
}

/// Approving a proposal with an InfisicalVaultStore causes the approval
/// service to POST the plaintext to the Infisical API.
#[tokio::test]
async fn approve_with_infisical_store_calls_infisical_api() {
    let infisical = httpmock::MockServer::start_async().await;
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_inf0001");
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_inf0001"}}));
        })
        .await;

    let (js, nats, _c) = start_nats().await;
    ensure_proposals_stream(&js).await.expect("proposals stream");

    let vault     = Arc::new(make_infisical_store(&infisical));
    let notifier  = Arc::new(NoopNotifier);
    let publisher = Arc::new(JetStreamStatePublisher::new(js.clone()));
    let svc = ApprovalService::new(vault, notifier, publisher, "prod");
    let (js_svc, nats_svc) = (js.clone(), nats.clone());
    tokio::spawn(async move { svc.run(js_svc, nats_svc).await.ok(); });
    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish(
        subjects::create("prod"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_inf0001",
            "credential_key": "tok_anthropic_prod_inf0001",
            "service":        "api.anthropic.com",
            "message":        "need access"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    nats.publish(
        subjects::approve("prod"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_inf0001",
            "approved_by": "mario",
            "plaintext":   "sk-ant-infisical-key"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    post_mock.assert_async().await;
}

/// When the vault is a DualWriteVault<InfisicalVaultStore, NatsKvVault>,
/// approving a proposal must write to both backends.  Verified by:
/// - Infisical POST mock was hit (primary written).
/// - Resolving via the shared Arc returns the value (cache written).
#[tokio::test]
async fn approve_with_dual_write_vault_writes_to_both_backends() {
    let infisical = httpmock::MockServer::start_async().await;
    let post_mock = infisical
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/anthropic_dw0002");
            then.status(200)
                .json_body(serde_json::json!({"secret": {"secretKey": "anthropic_dw0002"}}));
        })
        .await;

    let (js, nats, _c) = start_nats().await;
    ensure_proposals_stream(&js).await.expect("proposals stream");

    let nk_vault = make_nats_kv_vault(&js, "approvals-dual-test").await;
    let dual     = Arc::new(DualWriteVault::new(make_infisical_store(&infisical), nk_vault));
    let dual_check = Arc::clone(&dual);

    let notifier  = Arc::new(NoopNotifier);
    let publisher = Arc::new(JetStreamStatePublisher::new(js.clone()));
    let svc = ApprovalService::new(Arc::clone(&dual), notifier, publisher, "prod");
    let (js_svc, nats_svc) = (js.clone(), nats.clone());
    tokio::spawn(async move { svc.run(js_svc, nats_svc).await.ok(); });
    tokio::time::sleep(Duration::from_millis(100)).await;

    js.publish(
        subjects::create("prod"),
        serde_json::to_vec(&serde_json::json!({
            "id":             "prop_dw0002",
            "credential_key": "tok_anthropic_prod_dw0002",
            "service":        "api.anthropic.com",
            "message":        "dual write test"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    nats.publish(
        subjects::approve("prod"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_dw0002",
            "approved_by": "mario",
            "plaintext":   "sk-dual-write-key"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap();

    // Wait for approval processing and NatsKvVault watcher propagation.
    tokio::time::sleep(Duration::from_millis(400)).await;

    post_mock.assert_async().await;

    // Cache was written — resolve must return the approved value.
    // No Infisical GET mock is set up; a cache miss would return Ok(None).
    let token = ApiKeyToken::new("tok_anthropic_prod_dw0002").unwrap();
    let resolved = dual_check.resolve(&token).await.unwrap();
    assert_eq!(
        resolved,
        Some("sk-dual-write-key".to_string()),
        "NatsKvVault cache must hold the value after dual-write approval"
    );
}

/// Multiple proposals submitted and approved sequentially must all end up
/// in the vault with their correct individual values — no interference
/// between proposals.
#[tokio::test]
async fn multiple_proposals_all_approved_with_correct_values() {
    let (js, nats, _c) = start_nats().await;
    let vault = spawn_service(js.clone(), nats.clone(), "multi").await;

    const N: usize = 5;

    for i in 0..N {
        js.publish(
            subjects::create("multi"),
            serde_json::to_vec(&serde_json::json!({
                "id":             format!("prop_multi{i:04}"),
                "credential_key": format!("tok_anthropic_prod_mult{i:04}"),
                "service":        "api.anthropic.com",
                "message":        format!("proposal {i}")
            }))
            .unwrap()
            .into(),
        )
        .await
        .unwrap()
        .await
        .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(200)).await;

    for i in 0..N {
        nats.publish(
            subjects::approve("multi"),
            serde_json::to_vec(&serde_json::json!({
                "proposal_id": format!("prop_multi{i:04}"),
                "approved_by": "mario",
                "plaintext":   format!("sk-secret-{i}")
            }))
            .unwrap()
            .into(),
        )
        .await
        .unwrap();
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    for i in 0..N {
        let token = ApiKeyToken::new(&format!("tok_anthropic_prod_mult{i:04}")).unwrap();
        let resolved = vault.resolve(&token).await.unwrap();
        assert_eq!(
            resolved,
            Some(format!("sk-secret-{i}")),
            "proposal {i} must be in vault with correct plaintext"
        );
    }
}

#[tokio::test]
async fn malformed_create_message_is_nacked_and_service_continues() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "default").await;

    js.publish(
        subjects::create("default"),
        bytes::Bytes::from_static(b"not valid json at all"),
    )
    .await
    .expect("publish")
    .await
    .expect("jetstream ack");

    tokio::time::sleep(Duration::from_millis(200)).await;

    let resp = nats
        .request(subjects::status("default", "any"), bytes::Bytes::new())
        .await
        .expect("service must still be running after malformed message");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["status"], "not_found");
}
