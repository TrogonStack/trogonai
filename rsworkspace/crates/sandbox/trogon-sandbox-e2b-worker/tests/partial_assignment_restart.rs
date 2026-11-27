#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_process_support;
mod restart_support;
mod worker_process_support;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::{FutureExt, StreamExt};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use trogon_sandbox_contract::{ProviderKind, ProviderLease};
use trogon_sandbox_protocol::OP_LEASE_DISPATCH;

use ensure_support::{Boxed, Context};
use native_command_support::Guest;
use native_process_support::SANDBOX_ID;
use restart_support::{Facade, Fixture, Lease, counters, refuses};

const ASSIGNMENTS: &str = "sandbox-e2b-assignments";
const SENTINEL_EFFECT: &str = "partial-assignment-sentinel";

#[derive(PartialEq, Eq)]
struct Attempt {
    lease: String,
    phase: i64,
    intent: i64,
    started: bool,
    cleaned: bool,
    receipt: Option<Vec<u8>>,
    epoch: Option<String>,
}

/// Reads the worker's dispatch ledger without taking part in it.
fn attempt(path: &Path, lease: &Lease) -> Result<Attempt, Boxed> {
    let database = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let worker: String =
        database.query_row("SELECT value FROM metadata WHERE key='worker_id'", [], |row| row.get(0))?;
    let provider: String =
        database.query_row("SELECT value FROM metadata WHERE key='provider'", [], |row| row.get(0))?;
    assert!(
        worker == lease.target.worker_id.as_str() && provider == serde_json::to_string(&ProviderKind::E2b)?,
        "the read-only ledger observation must retain the exact worker and provider"
    );
    Ok(database.query_row(
        "SELECT lease_id,phase,intent,allocation_started,backend_cleaned,receipt,epoch FROM attempts WHERE attempt_id=?1",
        [lease.target.attempt_id.as_str()],
        |row| {
            Ok(Attempt {
                lease: row.get(0)?,
                phase: row.get(1)?,
                intent: row.get(2)?,
                started: row.get(3)?,
                cleaned: row.get(4)?,
                receipt: row.get(5)?,
                epoch: row.get(6)?,
            })
        },
    )?)
}

/// The partial allocation keeps started, unresolved authority, and a receipt only once the bind happened.
fn retained(attempt: &Attempt, lease: &Lease, bound: bool) -> Result<(), Boxed> {
    assert!(
        attempt.lease == lease.id.as_str()
            && attempt.started
            && !attempt.cleaned
            && attempt.intent == 0
            && attempt.epoch.as_deref() == Some("7"),
        "a partial allocation must retain exact started and unresolved ledger authority"
    );
    if bound {
        let receipt: ProviderLease = serde_json::from_slice(
            attempt
                .receipt
                .as_deref()
                .context("the backend receipt did not reach SQLite")?,
        )?;
        assert!(
            receipt.sandbox_id.as_str() == SANDBOX_ID
                && receipt.worker_id.as_ref() == Some(&lease.target.worker_id)
                && receipt.attempt_id.as_ref() == Some(&lease.target.attempt_id)
                && receipt.epoch == lease.target.epoch,
            "the retained receipt must carry only the exact signed native assignment"
        );
    } else {
        assert!(
            attempt.receipt.is_none(),
            "an unreturned native response must not invent a backend receipt"
        );
    }
    Ok(())
}

/// Neither native resource may have run a command, and the unrelated one keeps its effect.
fn native_controls(guest: &Guest, sentinel: &Guest) -> Result<(), Boxed> {
    for owned in [guest, sentinel] {
        assert!(
            owned.bootstrap_calls.load(Ordering::SeqCst) == 0
                && owned
                    .bindings
                    .lock()
                    .map_err(|_| "the binding record is poisoned")?
                    .is_empty(),
            "an ungranted partial allocation must not bootstrap any command"
        );
    }
    assert!(
        sentinel.effect(SENTINEL_EFFECT)?.is_some(),
        "the unrelated native sentinel effect must remain intact"
    );
    Ok(())
}

async fn partial_restart(bound: bool) -> Result<(), Boxed> {
    let fixture = Fixture::start("partial-assignment-fixture", 89).await?;
    let sentinel = Guest::create()?;
    std::fs::write(sentinel.path(SENTINEL_EFFECT), b"")?;
    let mut gate = Facade::start(&fixture.api.url, Arc::new(|_, _, _| Some(())), true).await?;
    let (mut worker, healthy) = fixture.boot(&gate.url, &fixture.native.url).await?;
    assert!(
        healthy.supports_lease_epoch.is_supported() && healthy.capacity.current_leases == 0,
        "the producer must start with epoch authority and empty capacity"
    );
    let lease = Lease::new(&healthy, "partial-assignment", &[3000])?;
    let body = fixture
        .signer
        .sign_payload(&serde_json::to_vec(&lease.dispatch)?, OP_LEASE_DISPATCH)?;
    let inbox = fixture.nats.client.new_inbox();
    let mut replies = fixture.nats.client.subscribe(inbox.clone()).await?;
    fixture.nats.client.flush().await?;
    let subject = lease.dispatch_subject();
    fixture
        .nats
        .client
        .publish_with_reply(subject.clone(), inbox.clone(), body.clone().into())
        .await?;

    gate.wait_created().await?;
    let store = fixture.context.get_key_value(ASSIGNMENTS).await?;
    let key = lease.assignment_key()?;
    let original = store
        .entry(&key)
        .await?
        .context("the pre-reserved recovery record is missing")?;
    let original_value: Value = serde_json::from_slice(&original.value)?;
    assert!(
        original_value["sandbox_id"].is_null()
            && original_value["recovery"] == json!({"exposed_ports":[3000],"ttl_seconds":600,"region":null})
            && original_value["tenant"] == json!(lease.scope.tenant),
        "the native create must follow a matching unbound recovery reservation"
    );
    let before = attempt(&fixture.ledger, &lease)?;
    retained(&before, &lease, false)?;
    assert!(
        before.phase == 0 && replies.next().now_or_never().is_none(),
        "the held create response must leave the attempt Starting with no ready grant"
    );
    native_controls(&fixture.guest, &sentinel)?;

    if bound {
        let mut native_stream = store.stream.clone();
        let config = native_stream.info().await?.config.clone();
        let mut constrained = config.clone();
        constrained.max_message_size = 1;
        fixture.context.update_stream(constrained.clone()).await?;
        let mut accepted = fixture.context.get_stream(&constrained.name).await?;
        assert!(
            serde_json::to_value(&accepted.info().await?.config)? == serde_json::to_value(&constrained)?
                && store
                    .entry(&key)
                    .await?
                    .is_some_and(|entry| entry.revision == original.revision && entry.value == original.value),
            "NATS must accept only the bind-size constraint and preserve the original authority"
        );
        gate.release();
        let refused = tokio::time::timeout(Duration::from_secs(10), replies.next())
            .await?
            .context("the failed bind did not return a dispatch refusal")?;
        let refused: Value = serde_json::from_slice(&refused.payload)?;
        assert!(
            serde_json::from_value::<ProviderLease>(refused.clone()).is_err()
                && refused["provider_error"]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
            "a failed native bind must not grant the lease: {refused}"
        );
        let captured = attempt(&fixture.ledger, &lease)?;
        retained(&captured, &lease, true)?;
        assert!(
            captured.phase == 0
                && store
                    .entry(&key)
                    .await?
                    .is_some_and(|entry| entry.revision == original.revision && entry.value == original.value),
            "a failed bind must retain Starting and must not rewrite the pre-reserved recovery"
        );
        fixture.context.update_stream(config).await?;
        fixture
            .nats
            .client
            .publish_with_reply(subject.clone(), inbox.clone(), body.clone().into())
            .await?;
        let replay = tokio::time::timeout(Duration::from_secs(10), replies.next())
            .await?
            .context("the unresolved replay did not refuse")?;
        let replay: Value = serde_json::from_slice(&replay.payload)?;
        assert!(
            serde_json::from_value::<ProviderLease>(replay.clone()).is_err()
                && replay["provider_error"].as_str().is_some_and(|value| !value.is_empty())
                && attempt(&fixture.ledger, &lease)? == captured,
            "restoring storage must not permit an unresolved replay or replace its durable receipt"
        );
    }
    let unresolved = attempt(&fixture.ledger, &lease)?;
    retained(&unresolved, &lease, bound)?;
    assert_eq!(
        counters(&fixture.api.control),
        [1, 0, 0, 0, 0, 0, 0, 0],
        "a partial allocation must have one native create without fallback or cleanup"
    );
    let fence = fixture.ledger.with_extension("fence");
    let original_fence = std::fs::read(&fence)?;
    let producer = worker.id();
    worker.stop()?;
    gate.release();
    assert!(
        attempt(&fixture.ledger, &lease)? == unresolved,
        "releasing a dead producer's response must not fabricate durable authority"
    );

    let (mut restarted, mut heartbeats) = fixture.spawn(&gate.url, &fixture.native.url).await?;
    assert!(restarted.id() != producer, "the restart must use a distinct process");
    refuses(&mut restarted, &mut heartbeats).await?;
    let after = attempt(&fixture.ledger, &lease)?;
    retained(&after, &lease, bound)?;
    let mut expected = unresolved;
    expected.phase = 2;
    assert!(
        after == expected,
        "the restart may quarantine but must not replace or erase the exact receipt"
    );
    let assignment = store.entry(&key).await?.context("the quarantine lost the assignment")?;
    let mut expected_value = original_value;
    if bound {
        expected_value["sandbox_id"] = json!(SANDBOX_ID);
    }
    let expected_revision = original.revision + u64::from(bound);
    assert!(
        serde_json::from_slice::<Value>(&assignment.value)? == expected_value
            && assignment.revision == expected_revision,
        "the quarantine may bind only the exact retained receipt, without synthesizing recovery or ownership"
    );
    assert!(
        std::fs::read(&fence)? == original_fence && counters(&fixture.api.control) == [1, 0, 0, 0, 0, 0, 0, 0],
        "the quarantine must keep the fence and never create, connect, list, get or delete a native resource"
    );
    native_controls(&fixture.guest, &sentinel)?;
    let product_removed = fixture.api.control.product_removed.load(Ordering::SeqCst);

    gate.shutdown().await;
    let cleanup = sentinel.cleanup();
    fixture.finish().await?;
    cleanup?;
    assert!(
        !product_removed,
        "a partial quarantine must not invent product deletion authority"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreturned_native_create_has_no_replay_or_guessed_cleanup_after_actual_restart() -> Result<(), Boxed> {
    partial_restart(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_native_bind_retains_exact_receipt_and_quarantine_after_actual_restart() -> Result<(), Boxed> {
    partial_restart(true).await
}
