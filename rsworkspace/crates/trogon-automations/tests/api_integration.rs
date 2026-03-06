//! HTTP API integration tests for trogon-automations.
//!
//! Spins up a real NATS container, an AutomationStore, and an Axum server,
//! then exercises every endpoint via reqwest.

use async_nats::jetstream;
use reqwest::Client;
use serde_json::{Value, json};
use testcontainers_modules::{nats::Nats, testcontainers::runners::AsyncRunner};
use tokio::net::TcpListener;
use trogon_automations::api::{AppState, router};
use trogon_automations::AutomationStore;

// ── Setup ─────────────────────────────────────────────────────────────────────

struct TestServer {
    base_url: String,
    client: Client,
    _container: Box<dyn std::any::Any>,
}

async fn start_server() -> TestServer {
    let container = Nats::default().start().await.expect("NATS container");
    let port = container.get_host_port_ipv4(4222).await.expect("port");
    let nats = async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .expect("NATS connect");
    let js = jetstream::new(nats);
    let store = AutomationStore::open(&js).await.expect("store");

    let state = AppState { store };
    let app = router(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("server") });

    TestServer {
        base_url: format!("http://{addr}"),
        client: Client::new(),
        _container: Box::new(container),
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
    let res = s.client.post(format!("{}/automations", s.base_url))
        .json(&create_body()).send().await.expect("send");

    assert_eq!(res.status(), 201);
    let body: Value = res.json().await.expect("json");
    assert!(!body["id"].as_str().unwrap_or("").is_empty());
    assert_eq!(body["name"], "PR review");
    assert_eq!(body["trigger"], "github.pull_request:opened");
    assert_eq!(body["enabled"], true);
}

#[tokio::test]
async fn list_returns_all_automations() {
    let s = start_server().await;

    s.client.post(format!("{}/automations", s.base_url)).json(&create_body()).send().await.unwrap();
    s.client.post(format!("{}/automations", s.base_url)).json(&json!({
        "name": "Push monitor",
        "trigger": "github.push",
        "prompt": "Check push.",
        "tools": [],
        "mcp_servers": []
    })).send().await.unwrap();

    let res = s.client.get(format!("{}/automations", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn get_existing_returns_200() {
    let s = start_server().await;
    let create_res: Value = s.client.post(format!("{}/automations", s.base_url))
        .json(&create_body()).send().await.unwrap().json().await.unwrap();
    let id = create_res["id"].as_str().unwrap();

    let res = s.client.get(format!("{}/automations/{id}", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], id);
}

#[tokio::test]
async fn get_missing_returns_404() {
    let s = start_server().await;
    let res = s.client.get(format!("{}/automations/does-not-exist", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn update_returns_updated_fields() {
    let s = start_server().await;
    let create_res: Value = s.client.post(format!("{}/automations", s.base_url))
        .json(&create_body()).send().await.unwrap().json().await.unwrap();
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

    let res = s.client.put(format!("{}/automations/{id}", s.base_url))
        .json(&update).send().await.unwrap();
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
    let res = s.client.put(format!("{}/automations/no-such-id", s.base_url))
        .json(&update).send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn delete_returns_204() {
    let s = start_server().await;
    let create_res: Value = s.client.post(format!("{}/automations", s.base_url))
        .json(&create_body()).send().await.unwrap().json().await.unwrap();
    let id = create_res["id"].as_str().unwrap();

    let del = s.client.delete(format!("{}/automations/{id}", s.base_url)).send().await.unwrap();
    assert_eq!(del.status(), 204);

    let get = s.client.get(format!("{}/automations/{id}", s.base_url)).send().await.unwrap();
    assert_eq!(get.status(), 404);
}

#[tokio::test]
async fn delete_missing_returns_404() {
    let s = start_server().await;
    let res = s.client.delete(format!("{}/automations/ghost", s.base_url)).send().await.unwrap();
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
    let created: Value = s.client.post(format!("{}/automations", s.base_url))
        .json(&body).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();

    let res = s.client.patch(format!("{}/automations/{id}/enable", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let resp: Value = res.json().await.unwrap();
    assert_eq!(resp["enabled"], true);
}

#[tokio::test]
async fn disable_sets_enabled_false() {
    let s = start_server().await;
    let created: Value = s.client.post(format!("{}/automations", s.base_url))
        .json(&create_body()).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();
    assert_eq!(created["enabled"], true);

    let res = s.client.patch(format!("{}/automations/{id}/disable", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let resp: Value = res.json().await.unwrap();
    assert_eq!(resp["enabled"], false);
}

#[tokio::test]
async fn enable_missing_returns_404() {
    let s = start_server().await;
    let res = s.client.patch(format!("{}/automations/no-id/enable", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 404);
}

#[tokio::test]
async fn list_empty_returns_empty_array() {
    let s = start_server().await;
    let res = s.client.get(format!("{}/automations", s.base_url)).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body, json!([]));
}

#[tokio::test]
async fn created_at_preserved_on_update() {
    let s = start_server().await;
    let created: Value = s.client.post(format!("{}/automations", s.base_url))
        .json(&create_body()).send().await.unwrap().json().await.unwrap();
    let id = created["id"].as_str().unwrap();
    let created_at = created["created_at"].as_str().unwrap().to_string();

    let update = json!({
        "name": "Changed", "trigger": "github.push", "prompt": "p",
        "tools": [], "mcp_servers": [], "enabled": true
    });
    let updated: Value = s.client.put(format!("{}/automations/{id}", s.base_url))
        .json(&update).send().await.unwrap().json().await.unwrap();

    assert_eq!(updated["created_at"], created_at, "created_at must not change on update");
}
