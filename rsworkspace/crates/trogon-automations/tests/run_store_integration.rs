//! NATS integration tests for [`RunStore`].
//!
//! Exercises the full KV round-trip: record → list → stats → tenant isolation.

use async_nats::jetstream;
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use trogon_automations::{RunRecord, RunStatus, RunStore, now_unix};

async fn make_run_store() -> (RunStore, impl Drop) {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = jetstream::new(nats);
    let store = RunStore::open(&js).await.expect("RunStore::open");
    (store, container)
}

fn run(id: &str, automation_id: &str, tenant_id: &str, status: RunStatus) -> RunRecord {
    let t = now_unix();
    RunRecord {
        id: id.to_string(),
        automation_id: automation_id.to_string(),
        automation_name: format!("Automation {automation_id}"),
        tenant_id: tenant_id.to_string(),
        nats_subject: "github.pull_request".to_string(),
        started_at: t,
        finished_at: t + 2,
        status,
        output: "ok".to_string(),
    }
}

// ── Basic CRUD ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn record_and_list_returns_all_runs() {
    let (store, _c) = make_run_store().await;
    store
        .record(&run("r1", "auto-1", "acme", RunStatus::Success))
        .await
        .expect("record r1");
    store
        .record(&run("r2", "auto-1", "acme", RunStatus::Failed))
        .await
        .expect("record r2");
    store
        .record(&run("r3", "auto-2", "acme", RunStatus::Success))
        .await
        .expect("record r3");

    let list = store.list("acme", None).await.expect("list");
    assert_eq!(list.len(), 3);
}

#[tokio::test]
async fn list_empty_when_no_runs() {
    let (store, _c) = make_run_store().await;
    let list = store.list("acme", None).await.expect("list");
    assert!(list.is_empty());
}

#[tokio::test]
async fn list_filter_by_automation_id() {
    let (store, _c) = make_run_store().await;
    store
        .record(&run("r1", "auto-A", "acme", RunStatus::Success))
        .await
        .unwrap();
    store
        .record(&run("r2", "auto-A", "acme", RunStatus::Success))
        .await
        .unwrap();
    store
        .record(&run("r3", "auto-B", "acme", RunStatus::Failed))
        .await
        .unwrap();

    let a_runs = store.list("acme", Some("auto-A")).await.expect("list A");
    assert_eq!(a_runs.len(), 2, "filter by auto-A must return 2 runs");
    assert!(a_runs.iter().all(|r| r.automation_id == "auto-A"));

    let b_runs = store.list("acme", Some("auto-B")).await.expect("list B");
    assert_eq!(b_runs.len(), 1, "filter by auto-B must return 1 run");
}

#[tokio::test]
async fn list_filter_returns_empty_for_unknown_automation() {
    let (store, _c) = make_run_store().await;
    store
        .record(&run("r1", "auto-A", "acme", RunStatus::Success))
        .await
        .unwrap();

    let missing = store
        .list("acme", Some("auto-MISSING"))
        .await
        .expect("list");
    assert!(missing.is_empty());
}

// ── Tenant isolation ───────────────────────────────────────────────────────────

#[tokio::test]
async fn list_tenant_isolation() {
    let (store, _c) = make_run_store().await;
    store
        .record(&run("r1", "auto-1", "acme", RunStatus::Success))
        .await
        .unwrap();
    store
        .record(&run("r2", "auto-1", "other", RunStatus::Success))
        .await
        .unwrap();

    let acme = store.list("acme", None).await.expect("acme");
    let other = store.list("other", None).await.expect("other");

    assert_eq!(acme.len(), 1);
    assert_eq!(acme[0].tenant_id, "acme");
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].tenant_id, "other");
}

// ── Stats ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn stats_empty_returns_zeros() {
    let (store, _c) = make_run_store().await;
    let stats = store.stats("acme").await.expect("stats");
    assert_eq!(stats.total, 0);
    assert_eq!(stats.successful_7d, 0);
    assert_eq!(stats.failed_7d, 0);
}

#[tokio::test]
async fn stats_counts_success_and_failed() {
    let (store, _c) = make_run_store().await;
    store
        .record(&run("r1", "a", "acme", RunStatus::Success))
        .await
        .unwrap();
    store
        .record(&run("r2", "a", "acme", RunStatus::Success))
        .await
        .unwrap();
    store
        .record(&run("r3", "a", "acme", RunStatus::Failed))
        .await
        .unwrap();

    let stats = store.stats("acme").await.expect("stats");
    assert_eq!(stats.total, 3);
    assert_eq!(stats.successful_7d, 2);
    assert_eq!(stats.failed_7d, 1);
}

#[tokio::test]
async fn stats_tenant_isolation() {
    let (store, _c) = make_run_store().await;
    store
        .record(&run("r1", "a", "acme", RunStatus::Success))
        .await
        .unwrap();
    store
        .record(&run("r2", "a", "other", RunStatus::Failed))
        .await
        .unwrap();

    let acme_stats = store.stats("acme").await.expect("acme stats");
    let other_stats = store.stats("other").await.expect("other stats");

    assert_eq!(acme_stats.total, 1);
    assert_eq!(acme_stats.successful_7d, 1);
    assert_eq!(acme_stats.failed_7d, 0);

    assert_eq!(other_stats.total, 1);
    assert_eq!(other_stats.successful_7d, 0);
    assert_eq!(other_stats.failed_7d, 1);
}

// ── Stats 7-day window ────────────────────────────────────────────────────────

#[tokio::test]
async fn stats_old_run_counts_in_total_but_not_in_7d() {
    let (store, _c) = make_run_store().await;
    let now = now_unix();

    // Recent success — inside 7d window.
    let mut recent = run("r-recent", "a", "acme", RunStatus::Success);
    recent.started_at = now - 3600; // 1 hour ago
    recent.finished_at = recent.started_at + 5;
    store.record(&recent).await.unwrap();

    // Old success — outside 7d window (8 days ago).
    let mut old = run("r-old", "a", "acme", RunStatus::Success);
    old.started_at = now.saturating_sub(8 * 86_400);
    old.finished_at = old.started_at + 5;
    store.record(&old).await.unwrap();

    let stats = store.stats("acme").await.expect("stats");
    assert_eq!(stats.total, 2, "total must include old runs");
    assert_eq!(
        stats.successful_7d, 1,
        "7d count must exclude the 8-day-old run"
    );
    assert_eq!(stats.failed_7d, 0);
}

#[tokio::test]
async fn stats_old_failed_run_excluded_from_7d() {
    let (store, _c) = make_run_store().await;
    let now = now_unix();

    // Old failure — outside 7d window.
    let mut old_fail = run("r-old-fail", "a", "acme", RunStatus::Failed);
    old_fail.started_at = now.saturating_sub(8 * 86_400);
    old_fail.finished_at = old_fail.started_at + 5;
    store.record(&old_fail).await.unwrap();

    let stats = store.stats("acme").await.expect("stats");
    assert_eq!(stats.total, 1, "total includes old run");
    assert_eq!(
        stats.failed_7d, 0,
        "old failure must not appear in 7d count"
    );
    assert_eq!(stats.successful_7d, 0);
}

// ── Run record fields ─────────────────────────────────────────────────────────

#[tokio::test]
async fn run_record_fields_preserved() {
    let (store, _c) = make_run_store().await;
    let mut r = run("r1", "auto-42", "acme", RunStatus::Success);
    r.automation_name = "My important automation".to_string();
    r.nats_subject = "cron.daily".to_string();
    r.output = "Reviewed 5 PRs".to_string();
    store.record(&r).await.expect("record");

    let list = store.list("acme", None).await.expect("list");
    assert_eq!(list.len(), 1);
    let got = &list[0];
    assert_eq!(got.id, "r1");
    assert_eq!(got.automation_id, "auto-42");
    assert_eq!(got.automation_name, "My important automation");
    assert_eq!(got.nats_subject, "cron.daily");
    assert_eq!(got.output, "Reviewed 5 PRs");
    assert_eq!(got.status, RunStatus::Success);
}

// ── Overwrite / idempotency ────────────────────────────────────────────────────

/// Recording the same run ID twice must overwrite the first record. The store
/// uses KV `put` (last-write-wins), so the second `record()` call replaces the
/// first — `list()` must return exactly one entry with the updated values.
#[tokio::test]
async fn record_overwrites_run_with_same_id() {
    let (store, _c) = make_run_store().await;

    let mut first = run("r-dup", "auto-1", "acme", RunStatus::Failed);
    first.output = "in progress".to_string();
    store.record(&first).await.expect("record first");

    // Second record with same ID — different status and output.
    let mut second = run("r-dup", "auto-1", "acme", RunStatus::Success);
    second.output = "completed successfully".to_string();
    store.record(&second).await.expect("record second");

    let list = store.list("acme", None).await.expect("list");
    assert_eq!(list.len(), 1, "duplicate ID must produce exactly one entry");
    assert_eq!(list[0].status, RunStatus::Success, "second record must overwrite status");
    assert_eq!(list[0].output, "completed successfully", "second record must overwrite output");
}

// ── Sort order ─────────────────────────────────────────────────────────────────

/// `list()` must return runs sorted newest-first (descending `started_at`).
/// The most recently started run must be at index 0.
#[tokio::test]
async fn list_returns_newest_first() {
    let (store, _c) = make_run_store().await;
    let base = now_unix();

    let mut old = run("r-old", "auto-1", "acme", RunStatus::Success);
    old.started_at = base - 3600; // 1 hour ago
    old.finished_at = old.started_at + 5;

    let mut mid = run("r-mid", "auto-1", "acme", RunStatus::Success);
    mid.started_at = base - 1800; // 30 min ago
    mid.finished_at = mid.started_at + 5;

    let mut recent = run("r-recent", "auto-1", "acme", RunStatus::Success);
    recent.started_at = base - 60; // 1 min ago
    recent.finished_at = recent.started_at + 5;

    // Insert in non-chronological order to confirm sort is applied.
    store.record(&mid).await.unwrap();
    store.record(&old).await.unwrap();
    store.record(&recent).await.unwrap();

    let list = store.list("acme", None).await.expect("list");
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].id, "r-recent", "newest run must be first");
    assert_eq!(list[1].id, "r-mid");
    assert_eq!(list[2].id, "r-old", "oldest run must be last");
}

// ── 7-day boundary ─────────────────────────────────────────────────────────────

/// A run at the exact 7-day boundary (started_at == now - 7*86400) is included
/// in the `successful_7d` / `failed_7d` counters — the window uses `>=` so the
/// boundary point counts. Only runs older than 7 days are excluded.
#[tokio::test]
async fn stats_run_at_exact_7d_boundary_included_in_window() {
    let (store, _c) = make_run_store().await;
    let now = now_unix();

    // Exactly 7 days ago — on the boundary, still inside the window (>=).
    let mut boundary = run("r-boundary", "auto-1", "acme", RunStatus::Success);
    boundary.started_at = now.saturating_sub(7 * 86_400);
    boundary.finished_at = boundary.started_at + 5;
    store.record(&boundary).await.unwrap();

    // 1 second further inside the window (6 days + 23h 59m 59s ago).
    let mut inside = run("r-inside", "auto-1", "acme", RunStatus::Success);
    inside.started_at = now.saturating_sub(7 * 86_400 - 1);
    inside.finished_at = inside.started_at + 5;
    store.record(&inside).await.unwrap();

    // 1 second outside the window (7 days + 1 second ago).
    let mut outside = run("r-outside", "auto-1", "acme", RunStatus::Success);
    outside.started_at = now.saturating_sub(7 * 86_400 + 1);
    outside.finished_at = outside.started_at + 5;
    store.record(&outside).await.unwrap();

    let stats = store.stats("acme").await.expect("stats");
    assert_eq!(stats.total, 3, "all three runs count in total");
    assert_eq!(
        stats.successful_7d, 2,
        "boundary and inside runs must count; only the run > 7 days old is excluded"
    );
}

/// `stats()` must skip corrupted (non-JSON) KV entries rather than returning
/// an error. The pull-consumer path silently drops records that fail
/// deserialization, so `stats()` must reflect only the valid entries.
#[tokio::test]
async fn stats_skips_invalid_json_entries() {
    use async_nats::jetstream;
    use testcontainers_modules::{nats::Nats, testcontainers::{ImageExt, runners::AsyncRunner}};

    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("connect");
    let js = jetstream::new(nats.clone());
    let store = RunStore::open(&js).await.expect("open");

    // Write a valid run.
    store.record(&run("r-good", "auto-1", "acme", RunStatus::Success))
        .await
        .unwrap();

    // Inject a corrupted entry directly into the KV bucket via raw NATS.
    // The KV bucket for RUNS is backed by a JetStream stream with subjects
    // "$KV.RUNS.{key}". We write garbage bytes using the kv API on the
    // underlying NATS connection.
    let kv = js.get_key_value("RUNS").await.expect("kv bucket");
    kv.put("acme.corrupted-run-id", bytes::Bytes::from(b"not valid json".to_vec()))
        .await
        .expect("inject corrupt entry");

    // stats() must not fail — it should count only the valid run.
    let stats = store.stats("acme").await.expect("stats must not error on corrupt entries");
    assert_eq!(stats.total, 1, "corrupted entry must be skipped, only valid run counted");
    assert_eq!(stats.successful_7d, 1);
}
