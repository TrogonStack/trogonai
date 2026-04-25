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
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_vault::{ApiKeyToken, MemoryVault, VaultStore};
use trogon_vault_approvals::{
    ApprovalService, NoopNotifier,
    ensure_proposals_stream,
    subjects,
};

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

    let vault    = Arc::new(MemoryVault::new());
    let notifier = Arc::new(NoopNotifier);
    let svc      = ApprovalService::new(
        Arc::clone(&vault),
        notifier,
        js,
        nats,
        vault_name,
    );

    tokio::spawn(async move { svc.run().await.ok(); });
    tokio::time::sleep(Duration::from_millis(100)).await;
    vault
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

    // Create the proposal.
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

    // Approve it with the plaintext key.
    js.publish(
        subjects::approve("prod"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_approve001",
            "approved_by": "mario",
            "plaintext":   "sk_live_supersecret"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Vault must contain the plaintext.
    let token = ApiKeyToken::new("tok_stripe_prod_abc123").unwrap();
    let resolved = vault.resolve(&token).await.unwrap();
    assert_eq!(resolved, Some("sk_live_supersecret".to_string()), "vault must hold plaintext after approval");

    // Status must be approved.
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

    js.publish(
        subjects::reject("staging"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_reject001",
            "rejected_by": "luigi",
            "reason":      "budget exceeded"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Vault must NOT contain the credential.
    let token = ApiKeyToken::new("tok_openai_staging_xyz789").unwrap();
    assert_eq!(vault.resolve(&token).await.unwrap(), None, "rejected proposal must not write vault");

    // Status must be rejected.
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

    // Publish an approve for a proposal that was never created.
    js.publish(
        subjects::approve("default"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_ghost",
            "approved_by": "ghost",
            "plaintext":   "sk_ghost"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    // Service must still be alive — a subsequent status query must work.
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

    // First approval
    js.publish(
        subjects::approve("prod"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_double001",
            "approved_by": "mario",
            "plaintext":   "sk-first"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    // Second approval (different plaintext) must be silently ignored.
    js.publish(
        subjects::approve("prod"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_double001",
            "approved_by": "luigi",
            "plaintext":   "sk-second"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Vault must hold the FIRST plaintext.
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

    // Create + approve in prod.
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

    js.publish(
        subjects::approve("prod"),
        serde_json::to_vec(&serde_json::json!({
            "proposal_id": "prop_iso001",
            "approved_by": "mario",
            "plaintext":   "sk-prod-key"
        }))
        .unwrap()
        .into(),
    )
    .await
    .unwrap()
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    // prod vault must have the key.
    assert_eq!(
        vault_prod.resolve(&token).await.unwrap(),
        Some("sk-prod-key".to_string())
    );
    // staging vault must not.
    assert_eq!(vault_staging.resolve(&token).await.unwrap(), None);
}

#[tokio::test]
async fn malformed_create_message_is_nacked_and_service_continues() {
    let (js, nats, _c) = start_nats().await;
    let _vault = spawn_service(js.clone(), nats.clone(), "default").await;

    // Publish garbage JSON to the create subject.
    js.publish(
        subjects::create("default"),
        bytes::Bytes::from_static(b"not valid json at all"),
    )
    .await
    .expect("publish")
    .await
    .expect("jetstream ack");

    tokio::time::sleep(Duration::from_millis(200)).await;

    // Service must still respond to status queries.
    let resp = nats
        .request(subjects::status("default", "any"), bytes::Bytes::new())
        .await
        .expect("service must still be running after malformed message");

    let v: serde_json::Value = serde_json::from_slice(&resp.payload).unwrap();
    assert_eq!(v["status"], "not_found");
}
