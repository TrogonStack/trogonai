//! NATS integration tests for [`RunStore`].
//!
//! Exercises the full KV round-trip: record → list → stats → tenant isolation.

use async_nats::jetstream;
use testcontainers_modules::{nats::Nats, testcontainers::{runners::AsyncRunner, ImageExt}};
use trogon_automations::{RunRecord, RunStatus, RunStore, now_unix};

async fn make_run_store() -> (RunStore, impl Drop) {
    let container = Nats::default().with_cmd(["--jetstream"]).start().await.expect("NATS");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}")).await.expect("connect");
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
    store.record(&run("r1", "auto-1", "acme", RunStatus::Success)).await.expect("record r1");
    store.record(&run("r2", "auto-1", "acme", RunStatus::Failed)).await.expect("record r2");
    store.record(&run("r3", "auto-2", "acme", RunStatus::Success)).await.expect("record r3");

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
    store.record(&run("r1", "auto-A", "acme", RunStatus::Success)).await.unwrap();
    store.record(&run("r2", "auto-A", "acme", RunStatus::Success)).await.unwrap();
    store.record(&run("r3", "auto-B", "acme", RunStatus::Failed)).await.unwrap();

    let a_runs = store.list("acme", Some("auto-A")).await.expect("list A");
    assert_eq!(a_runs.len(), 2, "filter by auto-A must return 2 runs");
    assert!(a_runs.iter().all(|r| r.automation_id == "auto-A"));

    let b_runs = store.list("acme", Some("auto-B")).await.expect("list B");
    assert_eq!(b_runs.len(), 1, "filter by auto-B must return 1 run");
}

#[tokio::test]
async fn list_filter_returns_empty_for_unknown_automation() {
    let (store, _c) = make_run_store().await;
    store.record(&run("r1", "auto-A", "acme", RunStatus::Success)).await.unwrap();

    let missing = store.list("acme", Some("auto-MISSING")).await.expect("list");
    assert!(missing.is_empty());
}

// ── Tenant isolation ───────────────────────────────────────────────────────────

#[tokio::test]
async fn list_tenant_isolation() {
    let (store, _c) = make_run_store().await;
    store.record(&run("r1", "auto-1", "acme", RunStatus::Success)).await.unwrap();
    store.record(&run("r2", "auto-1", "other", RunStatus::Success)).await.unwrap();

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
    store.record(&run("r1", "a", "acme", RunStatus::Success)).await.unwrap();
    store.record(&run("r2", "a", "acme", RunStatus::Success)).await.unwrap();
    store.record(&run("r3", "a", "acme", RunStatus::Failed)).await.unwrap();

    let stats = store.stats("acme").await.expect("stats");
    assert_eq!(stats.total, 3);
    assert_eq!(stats.successful_7d, 2);
    assert_eq!(stats.failed_7d, 1);
}

#[tokio::test]
async fn stats_tenant_isolation() {
    let (store, _c) = make_run_store().await;
    store.record(&run("r1", "a", "acme", RunStatus::Success)).await.unwrap();
    store.record(&run("r2", "a", "other", RunStatus::Failed)).await.unwrap();

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
    assert_eq!(stats.successful_7d, 1, "7d count must exclude the 8-day-old run");
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
    assert_eq!(stats.failed_7d, 0, "old failure must not appear in 7d count");
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
