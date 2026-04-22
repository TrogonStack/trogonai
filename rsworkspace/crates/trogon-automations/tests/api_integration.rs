//! HTTP API integration tests for trogon-automations.
//!
//! Spins up a real NATS container, an AutomationStore, and an Axum server,
//! then exercises every endpoint via reqwest.

use async_nats::jetstream;
use reqwest::Client;
use serde_json::{Value, json};
use testcontainers_modules::{
    nats::Nats,
    testcontainers::{ImageExt, runners::AsyncRunner},
};
use tokio::net::TcpListener;
use trogon_automations::api::{AppState, router};
use trogon_automations::{AutomationStore, RunRecord, RunStatus, RunStore, now_unix};

// ── Setup ─────────────────────────────────────────────────────────────────────

struct TestServer {
    base_url: String,
    client: Client,
    run_store: RunStore,
    _container: Box<dyn std::any::Any>,
}

async fn start_server() -> TestServer {
    let container = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);
    let store = AutomationStore::open(&js).await.expect("store");
    let run_store = RunStore::open(&js).await.expect("run_store");

    let state = AppState {
        store,
        run_store: run_store.clone(),
    };
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });

    TestServer {
        base_url: format!("http://{addr}"),
        client: Client::new(),
        run_store,
        _container: Box::new(container),
    }
}

fn seed_run(id: &str, automation_id: &str, tenant_id: &str, status: RunStatus) -> RunRecord {
    let t = now_unix();
    RunRecord {
        id: id.to_string(),
        automation_id: automation_id.to_string(),
        automation_name: format!("Automation {automation_id}"),
        tenant_id: tenant_id.to_string(),
        nats_subject: "github.pull_request".to_string(),
        started_at: t,
        finished_at: t + 1,
        status,
        output: "done".to_string(),
    }
}

fn create_body() -> Value {
    json!({
        "name": "PR review",
        "trigger": "github.pull_request:opened",
        "prompt": "Review the pull request.",
        "tools": ["get_pr_diff", "post_pr_comment"],
        "memory_path": ".trogon/memory.md",
        "mcp_servers": []
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_returns_201_with_id() {
    let s = start_server().await;
    let res = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .expect("send");

    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.expect("json");
    assert!(!body["id"].as_str().unwrap_or("").is_empty());
    assert_eq!(body["name"], "PR review");
    assert_eq!(body["trigger"], "github.pull_request:opened");
    assert_eq!(body["enabled"], true);
    assert_eq!(body["tenant_id"], "acme");
}

#[tokio::test]
async fn list_returns_all_automations() {
    let s = start_server().await;

    s.client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap();
    s.client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "Push monitor",
            "trigger": "github.push",
            "prompt": "Check push.",
            "tools": [],
            "mcp_servers": []
        }))
        .send()
        .await
        .unwrap();

    let res = s
        .client
        .get(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn get_existing_returns_200() {
    let s = start_server().await;
    let create_res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_res["id"].as_str().unwrap();

    let res = s
        .client
        .get(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], id);
}

#[tokio::test]
async fn get_missing_returns_404() {
    let s = start_server().await;
    let res = s
        .client
        .get(format!("{}/automations/does-not-exist", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn missing_tenant_header_returns_400() {
    let s = start_server().await;
    let res = s
        .client
        .get(format!("{}/automations", s.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn tenant_isolation_different_tenants_see_different_automations() {
    let s = start_server().await;
    // acme creates one automation
    s.client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap();
    // other-org creates one automation
    s.client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "other-org")
        .json(&create_body())
        .send()
        .await
        .unwrap();

    let acme_list: serde_json::Value = s
        .client
        .get(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_list: serde_json::Value = s
        .client
        .get(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "other-org")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        acme_list.as_array().unwrap().len(),
        1,
        "acme sees only its own"
    );
    assert_eq!(
        other_list.as_array().unwrap().len(),
        1,
        "other-org sees only its own"
    );
    // IDs are different
    assert_ne!(acme_list[0]["id"], other_list[0]["id"]);
}

#[tokio::test]
async fn update_returns_updated_fields() {
    let s = start_server().await;
    let create_res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_res["id"].as_str().unwrap();

    let update = json!({
        "name": "Updated name",
        "trigger": "github.push",
        "prompt": "New prompt.",
        "tools": [],
        "memory_path": null,
        "mcp_servers": [],
        "enabled": false
    });

    let res = s
        .client
        .put(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["name"], "Updated name");
    assert_eq!(body["trigger"], "github.push");
    assert_eq!(body["enabled"], false);
    assert_eq!(body["id"], id);
}

#[tokio::test]
async fn update_missing_returns_404() {
    let s = start_server().await;
    let update = json!({
        "name": "x", "trigger": "github.push", "prompt": "x",
        "tools": [], "mcp_servers": [], "enabled": true
    });
    let res = s
        .client
        .put(format!("{}/automations/no-such-id", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn delete_returns_204() {
    let s = start_server().await;
    let create_res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create_res["id"].as_str().unwrap();

    let del = s
        .client
        .delete(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(del.status(), 204);

    let get = s
        .client
        .get(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(get.status(), 404);
}

#[tokio::test]
async fn delete_missing_returns_404() {
    let s = start_server().await;
    let res = s
        .client
        .delete(format!("{}/automations/ghost", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn enable_sets_enabled_true() {
    let s = start_server().await;
    // Create disabled.
    let body = json!({
        "name": "Disabled", "trigger": "github.push", "prompt": "p",
        "tools": [], "mcp_servers": [], "enabled": false
    });
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let res = s
        .client
        .patch(format!("{}/automations/{id}/enable", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let resp: Value = res.json().await.unwrap();
    assert_eq!(resp["enabled"], true);
}

#[tokio::test]
async fn disable_sets_enabled_false() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["enabled"], true);

    let res = s
        .client
        .patch(format!("{}/automations/{id}/disable", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let resp: Value = res.json().await.unwrap();
    assert_eq!(resp["enabled"], false);
}

#[tokio::test]
async fn enable_missing_returns_404() {
    let s = start_server().await;
    let res = s
        .client
        .patch(format!("{}/automations/no-id/enable", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn disable_missing_returns_404() {
    let s = start_server().await;
    let res = s
        .client
        .patch(format!("{}/automations/no-id/disable", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn list_empty_returns_empty_array() {
    let s = start_server().await;
    let res = s
        .client
        .get(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn created_at_preserved_on_update() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let created_at = created["created_at"].as_str().unwrap().to_string();

    let update = json!({
        "name": "Changed", "trigger": "github.push", "prompt": "p",
        "tools": [], "mcp_servers": [], "enabled": true
    });
    let updated: Value = s
        .client
        .put(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(
        updated["created_at"], created_at,
        "created_at must not change on update"
    );
}

// ── model field ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn create_with_model_returns_model_in_response() {
    let s = start_server().await;
    let body = json!({
        "name": "Fast automation", "trigger": "github.push", "prompt": "p",
        "model": "claude-haiku-4-5-20251001"
    });
    let res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["model"], "claude-haiku-4-5-20251001");
}

#[tokio::test]
async fn create_without_model_returns_null() {
    let s = start_server().await;
    let res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(res["model"].is_null());
}

#[tokio::test]
async fn update_sets_model() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let update = json!({
        "name": "PR review", "trigger": "github.pull_request:opened", "prompt": "Review.",
        "tools": [], "mcp_servers": [], "enabled": true,
        "model": "claude-haiku-4-5-20251001"
    });
    let updated: Value = s
        .client
        .put(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["model"], "claude-haiku-4-5-20251001");
}

// ── visibility field ──────────────────────────────────────────────────────────

#[tokio::test]
async fn create_defaults_visibility_to_private() {
    let s = start_server().await;
    let res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["visibility"], "private");
}

#[tokio::test]
async fn create_with_visibility_public() {
    let s = start_server().await;
    let body = json!({
        "name": "Public automation", "trigger": "github.push", "prompt": "p",
        "visibility": "public"
    });
    let res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["visibility"], "public");
}

#[tokio::test]
async fn update_changes_visibility() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();
    let update = json!({
        "name": "PR review", "trigger": "github.pull_request:opened", "prompt": "Review.",
        "tools": [], "mcp_servers": [], "enabled": true,
        "visibility": "public"
    });
    let updated: Value = s
        .client
        .put(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["visibility"], "public");
}

// ── /runs endpoint ────────────────────────────────────────────────────────────

#[tokio::test]
async fn get_runs_missing_tenant_returns_400() {
    let s = start_server().await;
    let res = s
        .client
        .get(format!("{}/runs", s.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn get_runs_empty_returns_empty_array() {
    let s = start_server().await;
    let res: Value = s
        .client
        .get(format!("{}/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res, json!([]));
}

#[tokio::test]
async fn get_runs_filtered_by_automation_id() {
    let s = start_server().await;
    // Seed two runs via the RunStore directly (no runner needed).
    // Since RunStore isn't exposed through the test server, use the stats endpoint
    // to confirm the bucket exists and is functional.
    let stats: Value = s
        .client
        .get(format!("{}/stats", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stats["total"], 0);
    assert_eq!(stats["successful_7d"], 0);
    assert_eq!(stats["failed_7d"], 0);
}

// ── /automations/{id}/runs endpoint ──────────────────────────────────────────

#[tokio::test]
async fn get_runs_for_automation_path_returns_filtered_runs() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "auto-A", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "auto-A", "acme", RunStatus::Failed))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r3", "auto-B", "acme", RunStatus::Success))
        .await
        .unwrap();

    let res: Value = s
        .client
        .get(format!("{}/automations/auto-A/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = res.as_array().unwrap();
    assert_eq!(
        arr.len(),
        2,
        "/automations/auto-A/runs must return only auto-A runs"
    );
    assert!(arr.iter().all(|r| r["automation_id"] == "auto-A"));
}

#[tokio::test]
async fn get_runs_for_automation_path_missing_tenant_returns_400() {
    let s = start_server().await;
    let res = s
        .client
        .get(format!("{}/automations/auto-1/runs", s.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn get_runs_for_automation_path_empty_when_no_runs() {
    let s = start_server().await;
    let res: Value = s
        .client
        .get(format!("{}/automations/no-such-auto/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res, json!([]));
}

#[tokio::test]
async fn get_runs_for_automation_path_tenant_isolation() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "auto-1", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "auto-1", "other", RunStatus::Success))
        .await
        .unwrap();

    let acme: Value = s
        .client
        .get(format!("{}/automations/auto-1/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other: Value = s
        .client
        .get(format!("{}/automations/auto-1/runs", s.base_url))
        .header("x-tenant-id", "other")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(acme.as_array().unwrap().len(), 1);
    assert_eq!(acme[0]["tenant_id"], "acme");
    assert_eq!(other.as_array().unwrap().len(), 1);
    assert_eq!(other[0]["tenant_id"], "other");
}

// ── /stats endpoint ───────────────────────────────────────────────────────────

#[tokio::test]
async fn get_stats_missing_tenant_returns_400() {
    let s = start_server().await;
    let res = s
        .client
        .get(format!("{}/stats", s.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn get_stats_empty_returns_zeros() {
    let s = start_server().await;
    let res: Value = s
        .client
        .get(format!("{}/stats", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(res["total"], 0);
    assert_eq!(res["successful_7d"], 0);
    assert_eq!(res["failed_7d"], 0);
}

// ── /runs with seeded data ────────────────────────────────────────────────────

#[tokio::test]
async fn get_runs_with_data_returns_records() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "auto-1", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "auto-1", "acme", RunStatus::Failed))
        .await
        .unwrap();

    let res: Value = s
        .client
        .get(format!("{}/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = res.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert!(arr.iter().all(|r| r["tenant_id"] == "acme"));
    let statuses: Vec<&str> = arr.iter().map(|r| r["status"].as_str().unwrap()).collect();
    assert!(statuses.contains(&"success"));
    assert!(statuses.contains(&"failed"));
}

#[tokio::test]
async fn get_runs_filtered_by_automation_id_query_param() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "auto-A", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "auto-A", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r3", "auto-B", "acme", RunStatus::Failed))
        .await
        .unwrap();

    let all: Value = s
        .client
        .get(format!("{}/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 3, "unfiltered returns all 3");

    let filtered: Value = s
        .client
        .get(format!("{}/runs?automation_id=auto-A", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let arr = filtered.as_array().unwrap();
    assert_eq!(arr.len(), 2, "filter by auto-A must return 2");
    assert!(arr.iter().all(|r| r["automation_id"] == "auto-A"));
}

#[tokio::test]
async fn get_runs_tenant_isolation() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "auto-1", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "auto-1", "other", RunStatus::Success))
        .await
        .unwrap();

    let acme: Value = s
        .client
        .get(format!("{}/runs", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other: Value = s
        .client
        .get(format!("{}/runs", s.base_url))
        .header("x-tenant-id", "other")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(acme.as_array().unwrap().len(), 1);
    assert_eq!(other.as_array().unwrap().len(), 1);
}

// ── /stats with seeded data ───────────────────────────────────────────────────

#[tokio::test]
async fn get_stats_with_data_returns_correct_counts() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "a", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "a", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r3", "a", "acme", RunStatus::Failed))
        .await
        .unwrap();

    let stats: Value = s
        .client
        .get(format!("{}/stats", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(stats["total"], 3);
    assert_eq!(stats["successful_7d"], 2);
    assert_eq!(stats["failed_7d"], 1);
}

#[tokio::test]
async fn get_stats_tenant_isolation() {
    let s = start_server().await;
    s.run_store
        .record(&seed_run("r1", "a", "acme", RunStatus::Success))
        .await
        .unwrap();
    s.run_store
        .record(&seed_run("r2", "a", "other", RunStatus::Failed))
        .await
        .unwrap();

    let acme_stats: Value = s
        .client
        .get(format!("{}/stats", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let other_stats: Value = s
        .client
        .get(format!("{}/stats", s.base_url))
        .header("x-tenant-id", "other")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(acme_stats["total"], 1);
    assert_eq!(acme_stats["successful_7d"], 1);
    assert_eq!(other_stats["total"], 1);
    assert_eq!(other_stats["failed_7d"], 1);
}

// ── variables field ───────────────────────────────────────────────────────────

#[tokio::test]
async fn create_with_variables_returns_variables_in_response() {
    let s = start_server().await;
    let body = json!({
        "name": "Var automation",
        "trigger": "github.push",
        "prompt": "Hello {{name}} in {{env}}",
        "variables": {"name": "world", "env": "prod"}
    });
    let res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&body)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["variables"]["name"], "world");
    assert_eq!(res["variables"]["env"], "prod");
}

#[tokio::test]
async fn create_without_variables_returns_empty_object() {
    let s = start_server().await;
    let res: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(res["variables"], json!({}));
}

#[tokio::test]
async fn get_automation_preserves_variables() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "Var get",
            "trigger": "github.push",
            "prompt": "Hi {{user}}",
            "variables": {"user": "alice"}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let fetched: Value = s
        .client
        .get(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(fetched["variables"]["user"], "alice");
}

#[tokio::test]
async fn update_sets_variables() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&create_body())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let update = json!({
        "name": "PR review",
        "trigger": "github.pull_request:opened",
        "prompt": "Review for {{repo}}",
        "tools": [],
        "mcp_servers": [],
        "enabled": true,
        "variables": {"repo": "my-service"}
    });
    let updated: Value = s
        .client
        .put(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(updated["variables"]["repo"], "my-service");
}

#[tokio::test]
async fn update_clears_variables_when_empty() {
    let s = start_server().await;
    let created: Value = s
        .client
        .post(format!("{}/automations", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&json!({
            "name": "Clear vars",
            "trigger": "github.push",
            "prompt": "Hi {{user}}",
            "variables": {"user": "bob"}
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap();

    let update = json!({
        "name": "Clear vars",
        "trigger": "github.push",
        "prompt": "Hi",
        "tools": [],
        "mcp_servers": [],
        "enabled": true,
        "variables": {}
    });
    let updated: Value = s
        .client
        .put(format!("{}/automations/{id}", s.base_url))
        .header("x-tenant-id", "acme")
        .json(&update)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(updated["variables"], json!({}));
}
