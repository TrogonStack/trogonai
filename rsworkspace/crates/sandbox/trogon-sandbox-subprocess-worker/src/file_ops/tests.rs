use std::error::Error;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use trogon_sandbox_contract::{FileOpRequest, FileOpResponse, LeaseId};

use super::WorkspaceRegistry;

type TestResult = Result<(), Box<dyn Error>>;

fn request(lease: &str, op: Value) -> Result<FileOpRequest, serde_json::Error> {
    serde_json::from_value(json!({ "lease_id": lease, "op": op }))
}

fn write(path: &str, data: &[u8]) -> Value {
    json!({
        "op": "write",
        "path": path,
        "data": STANDARD.encode(data),
        "offset": 0,
        "truncate": true,
        "create_parents": true,
    })
}

async fn run(registry: &WorkspaceRegistry, lease: &str, op: Value) -> Result<Value, Box<dyn Error>> {
    let reply = registry
        .execute(&request(lease, op)?)
        .await
        .ok_or("the lease should answer")?;
    Ok(serde_json::to_value(reply)?)
}

#[tokio::test]
async fn a_lease_this_worker_does_not_own_gets_no_answer() -> TestResult {
    let registry = WorkspaceRegistry::new();
    let reply = registry
        .execute(&request("ghost", json!({"op": "stat", "path": "."}))?)
        .await;
    assert!(reply.is_none());
    Ok(())
}

#[tokio::test]
async fn operations_run_against_the_workspace_under_the_sandbox_path() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("l1"), dir.path().to_path_buf(), false);

    let written = run(&registry, "l1", write("/workspace/src/a.txt", b"hello")).await?;
    assert_eq!(written, json!({"outcome": "written", "bytes": 5, "size": 5}));
    assert_eq!(std::fs::read(dir.path().join("src/a.txt"))?, b"hello");

    let edited = run(
        &registry,
        "l1",
        json!({"op": "edit", "path": "src/a.txt", "find": "hello", "replace": "bye", "expected_occurrences": 1}),
    )
    .await?;
    assert_eq!(edited, json!({"outcome": "edited", "occurrences": 1, "size": 3}));
    Ok(())
}

#[tokio::test]
async fn paths_outside_the_workspace_are_refused() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("l1"), dir.path().to_path_buf(), false);
    for path in ["/etc/passwd", "../outside", "/workspace/../outside"] {
        let reply = run(&registry, "l1", json!({"op": "read", "path": path})).await?;
        assert_eq!(reply["outcome"], "failed", "{path}: {reply}");
        assert_eq!(reply["kind"], "invalid_path", "{path}: {reply}");
    }
    Ok(())
}

#[tokio::test]
async fn a_readonly_workspace_refuses_mutations_but_serves_reads() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    std::fs::write(dir.path().join("f.txt"), "x")?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("ro"), dir.path().to_path_buf(), true);

    let denied = run(&registry, "ro", write("f.txt", b"y")).await?;
    assert_eq!(denied["outcome"], "failed");
    assert_eq!(denied["kind"], "permission_denied");
    assert_eq!(std::fs::read(dir.path().join("f.txt"))?, b"x");

    let stat = run(&registry, "ro", json!({"op": "stat", "path": "f.txt"})).await?;
    assert_eq!(stat["outcome"], "stat");
    Ok(())
}

#[tokio::test]
async fn running_as_another_user_is_unsupported() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("l1"), dir.path().to_path_buf(), false);
    let req: FileOpRequest = serde_json::from_value(json!({
        "lease_id": "l1",
        "op": {"op": "stat", "path": "."},
        "as_user": "agent",
    }))?;
    let reply = registry.execute(&req).await.ok_or("the lease should answer")?;
    assert!(matches!(reply, FileOpResponse::Unsupported { .. }));
    Ok(())
}

#[tokio::test]
async fn changes_since_lease_start_and_since_a_checkpoint_follow_the_workspace() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    std::fs::write(dir.path().join("seed.txt"), "seed")?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("l1"), dir.path().to_path_buf(), false);

    run(&registry, "l1", write("/workspace/new.txt", b"n")).await?;
    std::fs::remove_file(dir.path().join("seed.txt"))?;
    let since_start = run(&registry, "l1", json!({"op": "changes"})).await?;
    assert_eq!(
        since_start,
        json!({
            "outcome": "changes",
            "since": "start",
            "added": ["/workspace/new.txt"],
            "modified": [],
            "deleted": ["/workspace/seed.txt"],
            "truncated": false,
        })
    );

    let checkpoint = run(
        &registry,
        "l1",
        json!({"op": "checkpoint", "path": "/workspace", "hash": true}),
    )
    .await?;
    let checkpoint_id = checkpoint["checkpoint_id"]
        .as_str()
        .ok_or("expected a checkpoint")?
        .to_string();
    run(&registry, "l1", write("/workspace/new.txt", b"changed")).await?;
    let after = run(&registry, "l1", json!({"op": "changes", "since": checkpoint_id})).await?;
    assert_eq!(
        after,
        json!({
            "outcome": "changes",
            "since": checkpoint_id,
            "added": [],
            "modified": ["/workspace/new.txt"],
            "deleted": [],
            "truncated": false,
        })
    );
    Ok(())
}

#[tokio::test]
async fn a_checkpoint_from_one_lease_is_unknown_to_another() -> TestResult {
    let a = tempfile::TempDir::new()?;
    let b = tempfile::TempDir::new()?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("a"), a.path().to_path_buf(), false);
    registry.insert(&LeaseId::new("b"), b.path().to_path_buf(), false);
    let checkpoint = run(
        &registry,
        "a",
        json!({"op": "checkpoint", "path": "/workspace", "hash": false}),
    )
    .await?;
    let checkpoint_id = checkpoint["checkpoint_id"].as_str().ok_or("expected a checkpoint")?;
    let reply = run(&registry, "b", json!({"op": "changes", "since": checkpoint_id})).await?;
    assert_eq!(reply["outcome"], "failed");
    assert_eq!(reply["kind"], "unknown_checkpoint");
    Ok(())
}

#[tokio::test]
async fn a_watch_reports_workspace_writes() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    let registry = WorkspaceRegistry::new();
    registry.insert(&LeaseId::new("l1"), dir.path().to_path_buf(), false);
    let started = run(
        &registry,
        "l1",
        json!({"op": "watch_start", "path": "/workspace", "recursive": true}),
    )
    .await?;
    let watch_id = started["watch_id"].as_str().ok_or("expected a watch")?;
    run(&registry, "l1", write("/workspace/d/a.txt", b"x")).await?;
    let polled = run(
        &registry,
        "l1",
        json!({"op": "watch_poll", "watch_id": watch_id, "after_seq": 0, "wait_ms": 1000}),
    )
    .await?;
    let events = polled["events"].as_array().ok_or("expected events")?;
    let paths: Vec<_> = events.iter().filter_map(|e| e["path"].as_str()).collect();
    assert_eq!(paths, vec!["/workspace/d", "/workspace/d/a.txt"]);
    Ok(())
}

#[tokio::test]
async fn removed_leases_stop_answering() -> TestResult {
    let dir = tempfile::TempDir::new()?;
    let registry = WorkspaceRegistry::new();
    let lease = LeaseId::new("l1");
    registry.insert(&lease, dir.path().to_path_buf(), false);
    assert!(registry.contains(&lease));
    registry.remove(&lease);
    assert!(
        registry
            .execute(&request("l1", json!({"op": "stat", "path": "."}))?)
            .await
            .is_none()
    );
    Ok(())
}
