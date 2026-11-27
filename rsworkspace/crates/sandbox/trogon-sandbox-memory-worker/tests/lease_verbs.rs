mod common;
mod nats_common;

use common::{TestResult, lease_request, start};
use serde_json::{Value, json};

fn text(value: &Value) -> String {
    value.to_string()
}

#[tokio::test]
async fn dispatch_refuses_what_it_cannot_honor() -> TestResult {
    let h = start().await?;
    let reply = h.dispatch_with("l1", json!({"nonsense": true})).await?;
    assert!(reply.get("decode_error").is_some());

    let mut request = lease_request();
    request["native"] = json!({"mock": {"label": "x"}});
    let granted = h.dispatch_with("l1", request).await?;
    assert_eq!(granted["worker_id"], h.worker.worker_id().as_str());
    assert_eq!(h.worker.lease_count(), 1);

    let mut request = lease_request();
    request["native"] = json!({"mock": {"other": 1}});
    let refused = h.dispatch_with("l2", request).await?;
    assert!(refused.get("sandbox_id").is_none());

    let reply = h.dispatch_with("bad lease id with spaces", lease_request()).await?;
    assert!(reply.get("decode_error").is_some() || reply.get("sandbox_id").is_some());
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn files_and_lifecycle_verbs_are_served() -> TestResult {
    let h = start().await?;
    let mut request = lease_request();
    request["region"] = json!("eu-1");
    h.dispatch_with("l1", request).await?;

    let handle = h.call_raw("native_handle", "l1", json!({"lease_id": "l1"})).await?;
    assert!(text(&handle).contains("memory-l1") && text(&handle).contains("eu-1"));

    let write = h
        .call_raw(
            "write_files",
            "l1",
            json!({"lease_id": "l1", "files": [{"path": "a/b.txt", "content": [1, 2, 3]}]}),
        )
        .await?;
    assert!(text(&write).contains("written"));
    let read = h
        .call_raw("read_file", "l1", json!({"lease_id": "l1", "path": "/a/b.txt"}))
        .await?;
    assert!(text(&read).contains("content"));
    let absent = h
        .call_raw("read_file", "l1", json!({"lease_id": "l1", "path": "/zzz"}))
        .await?;
    assert!(text(&absent).contains("not_found"));

    for (verb, body) in [
        ("poll_command", json!({})),
        ("kill_command", json!({})),
        ("stop_command", json!({})),
        ("restart_command", json!({})),
        ("list_processes", json!({})),
        ("wait_ready", json!({})),
        ("write_files", json!({})),
        ("read_file", json!({})),
        ("run_command", json!({})),
        ("suspend", json!({})),
        ("wake", json!({})),
    ] {
        let reply = h.call_raw(verb, "l1", body).await?;
        assert!(text(&reply).contains("invalid request"), "{verb}: {reply}");
    }
    let reply = h.call_raw("file_op", "l1", json!({})).await?;
    assert!(text(&reply).contains("invalid request"));

    let paused = h.call_raw("pause", "l1", json!({"lease_id": "l1"})).await?;
    assert!(text(&paused).contains("done"));
    h.call_raw("pause", "l1", json!({"lease_id": "l1"})).await?;
    for (verb, body) in [
        ("run_command", json!({"lease_id": "l1", "command": ["true"]})),
        ("write_files", json!({"lease_id": "l1", "files": []})),
        ("restart_command", json!({"lease_id": "l1", "execution_id": "x"})),
    ] {
        let reply = h.call_raw(verb, "l1", body).await?;
        assert!(text(&reply).contains("lease is paused"), "{verb}");
    }
    h.call_raw("resume", "l1", json!({"lease_id": "l1"})).await?;

    let suspend = |level: &str| json!({"lease_id": "l1", "generation": 1, "level": level});
    let reply = h.call_raw("suspend", "l1", suspend("memory")).await?;
    assert!(text(&reply).contains("done"));
    let reply = h.call_raw("suspend", "l1", suspend("disk")).await?;
    assert!(text(&reply).contains("unsupported"));
    let reply = h.call_raw("wake", "l1", suspend("memory")).await?;
    assert!(text(&reply).contains("done"));
    let reply = h.call_raw("wake", "l1", suspend("disk")).await?;
    assert!(text(&reply).contains("unsupported"));

    for verb in [
        "checkpoint_running",
        "network_policy_update",
        "write_stdin",
        "create_user",
        "create_group",
        "modify_group_membership",
        "get_domain",
    ] {
        let reply = h.call_raw(verb, "l1", json!({"lease_id": "l1"})).await?;
        assert!(!reply.is_null(), "{verb}");
    }
    h.worker.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn file_ops_track_changes_and_watches() -> TestResult {
    let h = start().await?;
    h.grant("l1").await?;
    let op = |op: Value| json!({"lease_id": "l1", "op": op});
    let checkpoint = h
        .call_raw(
            "file_op",
            "l1",
            op(json!({"op": "checkpoint", "path": "/", "hash": true})),
        )
        .await?;
    assert!(!text(&checkpoint).contains("failed"), "{checkpoint}");
    h.call_raw(
        "write_files",
        "l1",
        json!({"lease_id": "l1", "files": [{"path": "/w/f", "content": [1]}]}),
    )
    .await?;
    let changes = h.call_raw("file_op", "l1", op(json!({"op": "changes"}))).await?;
    assert!(text(&changes).contains("/w/f"), "{changes}");
    let watch = h
        .call_raw(
            "file_op",
            "l1",
            op(json!({"op": "watch_start", "path": "/", "recursive": true})),
        )
        .await?;
    let id = watch["watch_id"].clone();
    h.call_raw(
        "write_files",
        "l1",
        json!({"lease_id": "l1", "files": [{"path": "/w/g", "content": [2]}]}),
    )
    .await?;
    let poll = h
        .call_raw(
            "file_op",
            "l1",
            op(json!({"op": "watch_poll", "watch_id": id, "after_seq": 0, "wait_ms": 2000})),
        )
        .await?;
    assert!(text(&poll).contains("/w/g"), "{poll}");
    let unsupported = h
        .call_raw("file_op", "l1", op(json!({"op": "stat", "path": "/"})))
        .await?;
    assert!(text(&unsupported).contains("unsupported") || text(&unsupported).contains("failed"));
    h.worker.shutdown().await;
    Ok(())
}
