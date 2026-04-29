//! Binary end-to-end tests for the `approvals` binary.
//!
//! Spawns the real compiled `approvals` binary as an OS process, drives it
//! entirely through NATS (create / approve / reject / status), and asserts on
//! observable behaviour.  One test also spawns a mock Infisical server to
//! verify the correct HTTP call is made when the binary runs in Infisical mode.
//!
//! Because the approvals binary has no TCP port of its own, readiness is
//! detected by polling the NATS status request-reply until a response arrives.
//!
//! Requires Docker. Run with:
//!   cargo test -p trogon-secret-proxy --test binary_approvals_e2e

use std::time::Duration;

use bytes::Bytes;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use tokio::process::{Child, Command};

// ── Infrastructure helpers ────────────────────────────────────────────────────

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let container: ContainerAsync<Nats> = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = container.get_host_port_ipv4(4222).await.unwrap();
    (container, port)
}

fn spawn_approvals(nats_port: u16) -> Child {
    spawn_approvals_with_env(nats_port, std::iter::empty::<(&str, String)>())
}

fn spawn_approvals_with_env<'a>(
    nats_port:  u16,
    extra_env:  impl IntoIterator<Item = (&'a str, String)>,
) -> Child {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_approvals"));
    cmd.env("NATS_URL", format!("localhost:{nats_port}"))
        .env("RUST_LOG", "warn")
        .kill_on_drop(true);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.spawn()
        .expect("Failed to spawn approvals binary — run `cargo build` first")
}

/// Poll the status request-reply until the binary responds, confirming it is
/// alive and listening.  Even a "not_found" reply is sufficient evidence.
async fn wait_for_approvals_ready(nats: &async_nats::Client, vault_name: &str, timeout: Duration) {
    let subject  = format!("vault.proposals.{vault_name}.status.readiness_ping");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let got = tokio::time::timeout(
            Duration::from_millis(250),
            nats.request(subject.clone(), Bytes::new()),
        )
        .await;
        // Ok(Ok(_)) = service responded; anything else = not ready yet.
        // async-nats returns Ok(Err(NoResponders)) immediately when no subscriber
        // exists, so we must check the inner Result too, not just the outer one.
        if matches!(got, Ok(Ok(_))) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "approvals binary not ready within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ── NATS message helpers ──────────────────────────────────────────────────────

async fn nats_connect(port: u16) -> async_nats::Client {
    async_nats::connect(format!("localhost:{port}"))
        .await
        .expect("failed to connect to NATS")
}

async fn publish_create(nats: &async_nats::Client, vault: &str, id: &str, token: &str) {
    let subject = format!("vault.proposals.{vault}.create");
    let body    = serde_json::json!({
        "id":             id,
        "credential_key": token,
        "service":        "api.example.com",
        "message":        "binary e2e test",
    });
    nats.publish(subject, serde_json::to_vec(&body).unwrap().into())
        .await
        .unwrap();
    nats.flush().await.unwrap();
}

async fn publish_approve(
    nats:      &async_nats::Client,
    vault:     &str,
    id:        &str,
    by:        &str,
    plaintext: &str,
) {
    let subject = format!("vault.proposals.{vault}.approve");
    let body    = serde_json::json!({
        "proposal_id": id,
        "approved_by": by,
        "plaintext":   plaintext,
    });
    nats.publish(subject, serde_json::to_vec(&body).unwrap().into())
        .await
        .unwrap();
    nats.flush().await.unwrap();
}

async fn publish_reject(
    nats:   &async_nats::Client,
    vault:  &str,
    id:     &str,
    by:     &str,
    reason: &str,
) {
    let subject = format!("vault.proposals.{vault}.reject");
    let body    = serde_json::json!({
        "proposal_id": id,
        "rejected_by": by,
        "reason":      reason,
    });
    nats.publish(subject, serde_json::to_vec(&body).unwrap().into())
        .await
        .unwrap();
    nats.flush().await.unwrap();
}

async fn query_status(
    nats:  &async_nats::Client,
    vault: &str,
    id:    &str,
) -> serde_json::Value {
    let subject = format!("vault.proposals.{vault}.status.{id}");
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        nats.request(subject, Bytes::new()),
    )
    .await
    .expect("status query timed out");

    match result {
        Ok(msg) => serde_json::from_slice(&msg.payload).expect("status response is not valid JSON"),
        // No subscriber on this vault — binary isn't managing it.
        Err(_) => serde_json::json!({"status": "not_found", "proposal_id": id}),
    }
}

/// Retries [`query_status`] until `expected_status` is returned or the
/// deadline is reached.  Returns the full status JSON on success.
async fn poll_until_status(
    nats:            &async_nats::Client,
    vault:           &str,
    id:              &str,
    expected_status: &str,
    timeout:         Duration,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let v = query_status(nats, vault, id).await;
        if v["status"] == expected_status {
            return v;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for status={expected_status}, last response: {v}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Full happy path: create proposal → status pending → approve → status approved.
///
/// This is the core binary integration test — it drives the compiled binary
/// through its main execution path (MemoryVault mode) entirely via NATS.
#[tokio::test]
async fn binary_approvals_create_then_approve_status_shows_approved() {
    let (_nats, nats_port) = start_nats().await;
    let _proc              = spawn_approvals(nats_port);
    let nats               = nats_connect(nats_port).await;

    wait_for_approvals_ready(&nats, "default", Duration::from_secs(10)).await;

    // Create a proposal — published to the VAULT_PROPOSALS JetStream stream.
    publish_create(&nats, "default", "prop_bin_approve_001", "tok_stripe_prod_abc1").await;

    // Wait for the JetStream pull consumer to deliver and process the create.
    let pending = poll_until_status(&nats, "default", "prop_bin_approve_001", "pending", Duration::from_secs(5)).await;
    assert_eq!(pending["status"], "pending");

    // Approve the proposal via core NATS (intentionally not JetStream).
    publish_approve(&nats, "default", "prop_bin_approve_001", "mario", "sk_live_binary").await;

    // Service must transition to approved and update the in-memory store.
    let approved = poll_until_status(&nats, "default", "prop_bin_approve_001", "approved", Duration::from_secs(5)).await;
    assert_eq!(approved["status"],      "approved");
    assert_eq!(approved["approved_by"], "mario");
}

/// Reject path: create proposal → reject → status rejected with reason.
#[tokio::test]
async fn binary_approvals_create_then_reject_status_shows_rejected() {
    let (_nats, nats_port) = start_nats().await;
    let _proc              = spawn_approvals(nats_port);
    let nats               = nats_connect(nats_port).await;

    wait_for_approvals_ready(&nats, "default", Duration::from_secs(10)).await;

    publish_create(&nats, "default", "prop_bin_reject_001", "tok_openai_staging_abc1").await;
    poll_until_status(&nats, "default", "prop_bin_reject_001", "pending", Duration::from_secs(5)).await;

    publish_reject(&nats, "default", "prop_bin_reject_001", "luigi", "not authorised").await;

    let rejected = poll_until_status(&nats, "default", "prop_bin_reject_001", "rejected", Duration::from_secs(5)).await;
    assert_eq!(rejected["status"],      "rejected");
    assert_eq!(rejected["rejected_by"], "luigi");
    assert_eq!(rejected["reason"],      "not authorised");
}

/// Querying an unknown proposal ID returns `not_found` — the binary does not
/// panic and the service continues running.
#[tokio::test]
async fn binary_approvals_unknown_proposal_status_returns_not_found() {
    let (_nats, nats_port) = start_nats().await;
    let _proc              = spawn_approvals(nats_port);
    let nats               = nats_connect(nats_port).await;

    wait_for_approvals_ready(&nats, "default", Duration::from_secs(10)).await;

    let resp = query_status(&nats, "default", "prop_does_not_exist").await;
    assert_eq!(resp["status"],      "not_found");
    assert_eq!(resp["proposal_id"], "prop_does_not_exist");
}

/// `APPROVAL_VAULT_NAME` routes the service to a different NATS subject prefix.
/// Proposals sent to the default vault are not visible on the custom-named vault.
#[tokio::test]
async fn binary_approvals_custom_vault_name_routes_correctly() {
    let (_nats, nats_port) = start_nats().await;
    let _proc = spawn_approvals_with_env(
        nats_port,
        [("APPROVAL_VAULT_NAME", "staging".to_string())],
    );
    let nats = nats_connect(nats_port).await;

    // Readiness check on the custom vault name.
    wait_for_approvals_ready(&nats, "staging", Duration::from_secs(10)).await;

    // Proposal on the correct vault.
    publish_create(&nats, "staging", "prop_staging_001", "tok_stripe_staging_abc1").await;
    poll_until_status(&nats, "staging", "prop_staging_001", "pending", Duration::from_secs(5)).await;

    // That proposal must NOT appear on the default vault — different namespace.
    let resp = query_status(&nats, "default", "prop_staging_001").await;
    assert_eq!(resp["status"], "not_found",
        "proposal on 'staging' must be invisible to 'default' vault");
}

/// With `INFISICAL_URL` set (and `VAULT_NATS_BUCKET` absent), the binary selects
/// the Infisical-only vault backend.  After an approval, it must POST the
/// credential to Infisical — verified here against a mock HTTP server.
///
/// Token `tok_stripe_prod_abc1` maps to:
///   - Infisical secret name: `stripe_abc1`
///   - environment:           `prod`
///
/// Uses a unique vault name (`infisical_e2e`) so that approve messages from
/// other concurrently-running tests (which use `default`) cannot reach this
/// binary's subscription and cause spurious state transitions.
#[tokio::test]
async fn binary_approvals_infisical_mode_approve_posts_to_infisical() {
    let (_nats, nats_port) = start_nats().await;

    // Mock Infisical server — must exist before spawning the binary so the
    // URL is known at process start time.
    let mock_server = httpmock::MockServer::start_async().await;

    let store_mock = mock_server
        .mock_async(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/api/v3/secrets/raw/stripe_abc1")
                .json_body_partial(r#"{"environment":"prod","secretValue":"sk_live_infisical_mode"}"#);
            then.status(201)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({"secret": {"secretName": "stripe_abc1"}}));
        })
        .await;

    let _proc = spawn_approvals_with_env(
        nats_port,
        [
            ("APPROVAL_VAULT_NAME",     "infisical_e2e".to_string()),
            ("INFISICAL_URL",           mock_server.base_url()),
            ("INFISICAL_PROJECT_ID",    "proj_test_binary".to_string()),
            ("INFISICAL_SERVICE_TOKEN", "svc.binary.test".to_string()),
        ],
    );
    let nats = nats_connect(nats_port).await;

    wait_for_approvals_ready(&nats, "infisical_e2e", Duration::from_secs(10)).await;

    publish_create(&nats, "infisical_e2e", "prop_infisical_bin_001", "tok_stripe_prod_abc1").await;
    poll_until_status(&nats, "infisical_e2e", "prop_infisical_bin_001", "pending", Duration::from_secs(5)).await;

    publish_approve(
        &nats,
        "infisical_e2e",
        "prop_infisical_bin_001",
        "mario",
        "sk_live_infisical_mode",
    )
    .await;

    // Poll until approved — this confirms the vault.store() call to Infisical succeeded.
    let approved = poll_until_status(
        &nats,
        "infisical_e2e",
        "prop_infisical_bin_001",
        "approved",
        Duration::from_secs(5),
    )
    .await;
    assert_eq!(approved["approved_by"], "mario");

    // Verify the mock received exactly one POST request to Infisical.
    store_mock.assert_hits_async(1).await;
}
