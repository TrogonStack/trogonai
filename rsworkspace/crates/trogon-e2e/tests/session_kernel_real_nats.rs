//! Integration tests for the Session Kernel against a REAL NATS JetStream server.
//!
//! The kernel's core guarantees (monotonic `seq`, idempotent retry dedup, lease
//! contention, snapshot recovery/replay, portable-config preservation) are already
//! covered by `trogonai-session-kernel/tests/kernel_mock_nats.rs` using mock backends.
//! These tests re-assert the SAME guarantees against an actual JetStream + KV stack
//! provisioned by `SessionKernelStack::provision`, closing the "verified against mock
//! only" gap and mapping each test to an acceptance criterion of `cambio-modelo.md`.
//!
//! The lease KV bucket uses per-key TTL (limit markers), which requires NATS 2.11, so
//! these tests pin the `2.11-alpine` image. Requires Docker.

use buffa::{EnumValue, MessageField};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_cli::session_kernel::SessionKernelStack;
use trogonai_session_contracts::{
    Actor, ActorType, SCHEMA_VERSION_V1, SessionCreatedPayload, SessionEvent, SessionEventPayload, SessionId,
};
use trogonai_session_kernel::{
    SessionKernelError, SessionKernelFeatureFlags, SessionKernelOperationalPolicy, SessionMutatingOperation,
};

// ── helpers ─────────────────────────────────────────────────────────────────────

/// NATS 2.11+ JetStream server. The Session Kernel provisions a lease KV bucket with
/// per-key TTL, which requires NATS 2.11; the default testcontainers image is older.
async fn start_nats_js_v211() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .with_tag("2.11-alpine")
        .start()
        .await
        .expect("Failed to start NATS 2.11 container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn connect(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("failed to connect to NATS")
}

/// Provision a fresh kernel stack against the given NATS server. Defaults are
/// conservative; the kernel itself is enabled so append/materialize/lease work.
async fn provision_stack(nats: async_nats::Client) -> SessionKernelStack {
    let mut flags = SessionKernelFeatureFlags::default();
    flags.session_kernel_enabled = true;
    SessionKernelStack::provision(nats, flags, SessionKernelOperationalPolicy::default())
        .await
        .expect("kernel stack must provision against real NATS JetStream")
}

fn kernel_actor() -> Actor {
    Actor {
        r#type: EnumValue::Known(ActorType::Kernel),
        id: "session-kernel".to_string(),
        ..Actor::default()
    }
}

/// A `session_created` event with `seq = 0` (the kernel assigns the real seq on append).
/// `idempotency_key` is unique per event unless a retry is being simulated.
fn created_event(session_id: &str, idempotency_key: &str, compactor_model: Option<&str>) -> SessionEvent {
    SessionEvent {
        schema_version: SCHEMA_VERSION_V1,
        event_id: format!("evt_{idempotency_key}"),
        session_id: session_id.to_string(),
        seq: 0,
        operation_id: format!("op_{idempotency_key}"),
        correlation_id: format!("corr_{idempotency_key}"),
        idempotency_key: idempotency_key.to_string(),
        actor: MessageField::some(kernel_actor()),
        payload: MessageField::some(SessionEventPayload {
            kind: Some(
                SessionCreatedPayload {
                    title: "real-nats integration".to_string(),
                    cwd: "/repo".to_string(),
                    compactor_model: compactor_model.map(str::to_string),
                    ..SessionCreatedPayload::default()
                }
                .into(),
            ),
            ..SessionEventPayload::default()
        }),
        ..SessionEvent::default()
    }
}

// ── tests ───────────────────────────────────────────────────────────────────────

/// AC-2 (`seq` monótono) + AC-3/AC-24 (snapshot guarda `last_applied_seq`,
/// regenerable desde eventos): appending events over real JetStream assigns a
/// monotonically increasing per-session `seq`, and materializing reads it back.
#[tokio::test]
async fn append_event_assigns_monotonic_seq_over_real_nats() {
    let (_c, port) = start_nats_js_v211().await;
    let stack = provision_stack(connect(port).await).await;
    let sid = SessionId::new("sess_seq").unwrap();

    let first = stack
        .kernel
        .append_event(created_event("sess_seq", "idem_1", None))
        .await
        .expect("first append must succeed over real NATS");
    assert_eq!(first.seq, 1, "first event gets seq 1");

    let second = stack
        .kernel
        .append_event(created_event("sess_seq", "idem_2", None))
        .await
        .expect("second append must succeed");
    assert_eq!(second.seq, 2, "seq is monotonic per session");

    let materialized = stack.kernel.materialize_state(&sid).await.unwrap();
    assert_eq!(
        materialized.last_applied_seq, 2,
        "snapshot materialized from the event log records last_applied_seq"
    );
}

/// AC-4 (retries no duplican eventos): `append_event_idempotent` with the same key,
/// replayed after a simulated crash, returns the same event and the log holds exactly
/// one event — verified by replay over real JetStream.
#[tokio::test]
async fn idempotent_retry_does_not_duplicate_over_real_nats() {
    let (_c, port) = start_nats_js_v211().await;
    let stack = provision_stack(connect(port).await).await;
    let sid = SessionId::new("sess_idem").unwrap();

    let event = created_event("sess_idem", "idem_retry", None);
    let first = stack
        .kernel
        .append_event_idempotent(event.clone(), "idem_retry")
        .await
        .expect("first idempotent append must succeed");

    // Simulate a crash/retry after the durable append.
    let retry = stack
        .kernel
        .append_event_idempotent(event, "idem_retry")
        .await
        .expect("retry must be a safe no-op");

    assert_eq!(first.event_id, retry.event_id, "retry returns the same event_id");
    assert_eq!(first.seq, retry.seq, "retry returns the same seq (no new event)");

    let recovered = stack.kernel.recover(&sid).await.unwrap();
    assert_eq!(
        recovered.replayed_events, 1,
        "exactly one event in the log despite the retry — no duplication"
    );
}

/// AC-7 (no dos operaciones mutadoras simultáneas por `session_id`): a second lease
/// acquire while the first is held returns `SessionBusy` over the real lease KV bucket;
/// after release, the session can be re-acquired.
#[tokio::test]
async fn lease_contention_returns_session_busy_over_real_nats() {
    let (_c, port) = start_nats_js_v211().await;
    let stack = provision_stack(connect(port).await).await;
    let sid = SessionId::new("sess_busy").unwrap();

    let guard = stack
        .kernel
        .acquire_session_lease(&sid, SessionMutatingOperation::PromptTurn)
        .await
        .expect("first lease acquire must succeed");

    let contended = stack
        .kernel
        .acquire_session_lease(&sid, SessionMutatingOperation::SwitchModel)
        .await;
    let is_busy = matches!(contended, Err(SessionKernelError::SessionBusy { .. }));
    // Drop any guard returned by an (incorrect) success before asserting, so the lease
    // would be released either way; the assertion is what fails the test.
    drop(contended.ok());
    assert!(
        is_busy,
        "a concurrent mutating op on the same session must return SessionBusy"
    );

    stack.kernel.release_session_lease(guard).await.unwrap();

    // Once released, the lease is free again.
    let reacquired = stack
        .kernel
        .acquire_session_lease(&sid, SessionMutatingOperation::SwitchModel)
        .await
        .expect("lease must be re-acquirable after release");
    stack.kernel.release_session_lease(reacquired).await.unwrap();
}

/// AC-24/AC-3 (snapshots regenerables desde eventos tras un "reinicio"): events appended
/// by one kernel stack are recovered by a FRESH stack provisioned against the same NATS,
/// proving the event log — not in-process state — is the source of truth.
#[tokio::test]
async fn fresh_stack_recovers_snapshot_from_event_log_over_real_nats() {
    let (_c, port) = start_nats_js_v211().await;
    let nats = connect(port).await;
    let sid = SessionId::new("sess_recover").unwrap();

    {
        let stack = provision_stack(nats.clone()).await;
        stack
            .kernel
            .append_event(created_event("sess_recover", "idem_a", None))
            .await
            .unwrap();
        stack
            .kernel
            .append_event(created_event("sess_recover", "idem_b", None))
            .await
            .unwrap();
        // stack dropped here — simulates process restart.
    }

    let fresh = provision_stack(nats).await;
    let recovered = fresh.kernel.recover(&sid).await.unwrap();
    assert_eq!(recovered.replayed_events, 2, "a fresh stack replays both durable events");
    assert_eq!(
        recovered.snapshot.last_applied_seq, 2,
        "the snapshot regenerated from the event log is at the latest seq"
    );
}

/// AC-12 (`compactor_model` se preserva): a `session_created` carrying an explicit
/// `compactor_model` materializes it onto the canonical snapshot config over real NATS,
/// so a later main-model switch can preserve the user's compactor choice.
#[tokio::test]
async fn compactor_model_preserved_in_snapshot_over_real_nats() {
    let (_c, port) = start_nats_js_v211().await;
    let stack = provision_stack(connect(port).await).await;
    let sid = SessionId::new("sess_compactor").unwrap();

    stack
        .kernel
        .append_event(created_event(
            "sess_compactor",
            "idem_c",
            Some("xai/grok-code-fast"),
        ))
        .await
        .unwrap();

    let snapshot = stack.kernel.materialize_state(&sid).await.unwrap();
    let config = snapshot
        .state
        .as_option()
        .and_then(|s| s.config.as_option())
        .expect("snapshot must carry portable config");
    assert_eq!(
        config.compactor_model.as_deref(),
        Some("xai/grok-code-fast"),
        "compactor_model is preserved as portable session config"
    );
}
