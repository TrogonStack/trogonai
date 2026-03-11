//! Integration tests for AutomationStore — require a live NATS server.
//!
//! Run with:
//! ```sh
//! NATS_TEST_URL=nats://localhost:4222 cargo test -p trogon-automations
//! ```
//! or let the testcontainers setup spin up NATS automatically.

use async_nats::jetstream;
use testcontainers_modules::{nats::Nats, testcontainers::{runners::AsyncRunner, ImageExt}};
use trogon_automations::{Automation, AutomationStore, McpServer};

fn sample_automation(id: &str, trigger: &str) -> Automation {
    Automation {
        id: id.to_string(),
        tenant_id: "test-tenant".to_string(),
        name: format!("Test automation {id}"),
        trigger: trigger.to_string(),
        prompt: "Do something useful.".to_string(),
        model: None,
        tools: vec!["get_pr_diff".to_string()],
        memory_path: None,
        mcp_servers: vec![],
        enabled: true,
        visibility: trogon_automations::Visibility::Private,
        created_at: "2026-01-01T00:00:00Z".to_string(),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

async fn make_store() -> (AutomationStore, impl Drop) {
    let container = Nats::default().with_cmd(["--jetstream"]).start().await.expect("NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);
    let store = AutomationStore::open(&js).await.expect("open store");
    (store, container)
}

#[tokio::test]
async fn put_and_get_round_trips() {
    let (store, _c) = make_store().await;
    let a = sample_automation("a1", "github.pull_request:opened");
    store.put(&a).await.expect("put");
    let fetched = store.get("test-tenant", "a1").await.expect("get").expect("should exist");
    assert_eq!(fetched, a);
}

#[tokio::test]
async fn get_missing_returns_none() {
    let (store, _c) = make_store().await;
    let result = store.get("no-tenant", "nonexistent").await.expect("get");
    assert!(result.is_none());
}

#[tokio::test]
async fn delete_removes_entry() {
    let (store, _c) = make_store().await;
    let a = sample_automation("del1", "github.push");
    store.put(&a).await.expect("put");
    store.delete("test-tenant", "del1").await.expect("delete");
    assert!(store.get("test-tenant", "del1").await.expect("get").is_none());
}

#[tokio::test]
async fn list_returns_all_entries() {
    let (store, _c) = make_store().await;
    let a1 = sample_automation("l1", "github.push");
    let a2 = sample_automation("l2", "linear.Issue:create");
    store.put(&a1).await.expect("put a1");
    store.put(&a2).await.expect("put a2");

    let list = store.list("test-tenant").await.expect("list");
    assert_eq!(list.len(), 2);
    let ids: Vec<&str> = list.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&"l1"));
    assert!(ids.contains(&"l2"));
}

#[tokio::test]
async fn list_empty_store_returns_empty_vec() {
    let (store, _c) = make_store().await;
    let list = store.list("test-tenant").await.expect("list");
    assert!(list.is_empty());
}

#[tokio::test]
async fn put_overwrites_existing_entry() {
    let (store, _c) = make_store().await;
    let mut a = sample_automation("upd1", "github.push");
    store.put(&a).await.expect("put");

    a.name = "Updated name".to_string();
    store.put(&a).await.expect("put updated");

    let fetched = store.get("test-tenant", "upd1").await.expect("get").expect("exists");
    assert_eq!(fetched.name, "Updated name");
}

#[tokio::test]
async fn matching_returns_only_enabled_and_triggered() {
    let (store, _c) = make_store().await;

    let pr_open = sample_automation("m1", "github.pull_request:opened");
    let pr_any = sample_automation("m2", "github.pull_request");
    let mut disabled = sample_automation("m3", "github.pull_request:opened");
    disabled.enabled = false;
    let push = sample_automation("m4", "github.push");

    store.put(&pr_open).await.expect("put");
    store.put(&pr_any).await.expect("put");
    store.put(&disabled).await.expect("put");
    store.put(&push).await.expect("put");

    let payload = serde_json::json!({"action": "opened"});
    let matched = store.matching("test-tenant", "github.pull_request", &payload).await.expect("matching");

    let ids: Vec<&str> = matched.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&"m1"), "m1 should match");
    assert!(ids.contains(&"m2"), "m2 (any action) should match");
    assert!(!ids.contains(&"m3"), "m3 is disabled");
    assert!(!ids.contains(&"m4"), "m4 is wrong trigger");
}

// ── matching() — trigger variants ─────────────────────────────────────────────

#[tokio::test]
async fn matching_cron_wildcard_matches_any_cron_subject() {
    let (store, _c) = make_store().await;
    let cron_any = sample_automation("c1", "cron");
    store.put(&cron_any).await.expect("put");

    let matched = store
        .matching("test-tenant", "cron.daily-digest", &serde_json::json!({}))
        .await
        .expect("matching");

    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].id, "c1");
}

#[tokio::test]
async fn matching_cron_exact_matches_only_that_job() {
    let (store, _c) = make_store().await;
    store.put(&sample_automation("c2", "cron.my-job")).await.expect("put c2");
    store.put(&sample_automation("c3", "cron.other-job")).await.expect("put c3");

    let matched = store
        .matching("test-tenant", "cron.my-job", &serde_json::json!({}))
        .await
        .expect("matching");

    let ids: Vec<&str> = matched.iter().map(|a| a.id.as_str()).collect();
    assert!(ids.contains(&"c2"), "exact cron job must match");
    assert!(!ids.contains(&"c3"), "different cron job must not match");
}

#[tokio::test]
async fn matching_linear_issue_prefix_matches() {
    let (store, _c) = make_store().await;
    store
        .put(&sample_automation("l1", "linear.Issue:create"))
        .await
        .expect("put");

    let payload = serde_json::json!({"action": "create"});
    let matched = store
        .matching("test-tenant", "linear.Issue.create", &payload)
        .await
        .expect("matching");

    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].id, "l1");
}

#[tokio::test]
async fn matching_cross_tenant_returns_empty() {
    let (store, _c) = make_store().await;
    // Automation belongs to "test-tenant".
    store
        .put(&sample_automation("iso-m1", "github.push"))
        .await
        .expect("put");

    let payload = serde_json::json!({});
    let matched = store
        .matching("other-tenant", "github.push", &payload)
        .await
        .expect("matching");

    assert!(matched.is_empty(), "other-tenant must not match test-tenant's automations");
}

#[tokio::test]
async fn watch_delivers_snapshot_and_updates() {
    use futures_util::StreamExt as _;
    use tokio::time::{Duration, timeout};

    let (store, _c) = make_store().await;

    // Pre-populate one entry.
    let a1 = sample_automation("w1", "github.push");
    store.put(&a1).await.expect("put");

    let mut stream = store.watch().await.expect("watch");

    // Snapshot: w1 delivered immediately.
    let (key, auto) = timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended");
    assert_eq!(key, "test-tenant.w1");
    assert!(auto.is_some());

    // Write a second entry — should arrive as an incremental update.
    let a2 = sample_automation("w2", "linear.Issue:create");
    store.put(&a2).await.expect("put w2");

    let (key2, auto2) = timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout")
        .expect("stream ended");
    assert_eq!(key2, "test-tenant.w2");
    assert_eq!(auto2.unwrap().trigger, "linear.Issue:create");
}

#[tokio::test]
async fn watch_delivers_delete_as_none() {
    use futures_util::StreamExt as _;
    use tokio::time::{Duration, timeout};

    let (store, _c) = make_store().await;

    let a = sample_automation("wd1", "github.push");
    store.put(&a).await.expect("put");

    let mut stream = store.watch().await.expect("watch");

    // Consume snapshot.
    timeout(Duration::from_secs(3), stream.next()).await.expect("t").expect("s");

    // Delete and observe the None.
    store.delete("test-tenant", "wd1").await.expect("delete");
    let (key, auto) = timeout(Duration::from_secs(3), stream.next())
        .await
        .expect("timeout")
        .expect("stream");
    assert_eq!(key, "test-tenant.wd1");
    assert!(auto.is_none());
}

#[tokio::test]
async fn cross_tenant_get_returns_none() {
    let (store, _c) = make_store().await;
    let a = sample_automation("iso1", "github.push"); // tenant_id = "test-tenant"
    store.put(&a).await.expect("put");

    // A different tenant cannot see tenant-a's automation.
    let result = store.get("other-tenant", "iso1").await.expect("get");
    assert!(result.is_none(), "other-tenant must not see test-tenant's automation");
}

#[tokio::test]
async fn cross_tenant_list_returns_empty() {
    let (store, _c) = make_store().await;
    let a = sample_automation("iso2", "github.push"); // tenant_id = "test-tenant"
    store.put(&a).await.expect("put");

    let list = store.list("other-tenant").await.expect("list");
    assert!(list.is_empty(), "other-tenant must see an empty list");
}

#[tokio::test]
async fn cross_tenant_delete_does_not_remove_other_tenants_data() {
    let (store, _c) = make_store().await;
    let a = sample_automation("iso3", "github.push"); // tenant_id = "test-tenant"
    store.put(&a).await.expect("put");

    // Deleting with the wrong tenant has no effect (NATS KV key doesn't exist).
    store.delete("other-tenant", "iso3").await.expect("delete");

    // Original entry still exists for the correct tenant.
    let fetched = store.get("test-tenant", "iso3").await.expect("get");
    assert!(fetched.is_some(), "test-tenant's entry should still exist");
}

#[tokio::test]
async fn automation_with_mcp_servers_round_trips() {
    let (store, _c) = make_store().await;
    let mut a = sample_automation("mcp1", "github.push");
    a.mcp_servers = vec![
        McpServer { name: "search".to_string(), url: "http://localhost:3000".to_string() },
        McpServer { name: "db".to_string(), url: "http://localhost:3001".to_string() },
    ];
    store.put(&a).await.expect("put");
    let fetched = store.get("test-tenant", "mcp1").await.expect("get").expect("exists");
    assert_eq!(fetched.mcp_servers.len(), 2);
    assert_eq!(fetched.mcp_servers[0].name, "search");
}

/// `list()` must silently skip entries whose bytes cannot be deserialized,
/// warning rather than failing, and return the other valid automations.
#[tokio::test]
async fn list_skips_unreadable_entry_and_returns_valid_ones() {
    let container = Nats::default().with_cmd(["--jetstream"]).start().await.expect("NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);
    let store = AutomationStore::open(&js).await.expect("open store");

    // Insert one valid automation.
    let good = sample_automation("good-1", "github.push");
    store.put(&good).await.expect("put good");

    // Directly write corrupted bytes for a second key under the same tenant prefix.
    let kv = js
        .get_key_value(trogon_automations::store::BUCKET)
        .await
        .expect("get KV bucket");
    kv.put("test-tenant.corrupted", bytes::Bytes::from(b"not valid json" as &[u8]))
        .await
        .expect("inject bad bytes");

    // list() must return only the valid automation and silently skip the corrupt one.
    let result = store.list("test-tenant").await.expect("list");
    assert_eq!(result.len(), 1, "only the valid automation should be returned");
    assert_eq!(result[0].id, "good-1");
}
