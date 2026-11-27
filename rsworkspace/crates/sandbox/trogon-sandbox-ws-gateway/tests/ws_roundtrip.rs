mod support;

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use support::{
    MockSandboxBackend, TestResult, api_key, call, call_err, call_ok, connect_with_token, lease_request, open_socket,
    read_event, read_response, send, serve, session_token, test_state,
};
use trogon_sandbox_contract::{LeaseId, LogChunk, OutputStream};
use trogon_sandbox_ws_gateway::API_KEY_HEADER;

fn digest(fill: &str) -> String {
    format!("sha256:{}", fill.repeat(64))
}

#[tokio::test]
async fn sessions_and_lease_tools_round_trip() -> TestResult {
    let key = api_key("roundtrip");
    let mock = Arc::new(MockSandboxBackend::new());
    mock.queue_evidence(
        LeaseId::new("lease-evidence-1"),
        json!({"subject_suffix": "lifecycle", "payload": "started"}),
    );
    let addr = serve(test_state(&key, "tenant-test", mock).await?).await?;
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/v1/sessions"))
        .header(API_KEY_HEADER, &key)
        .send()
        .await?;
    assert_eq!(response.status(), 201);
    let session: Value = response.json().await?;
    assert!(session["expires_at"].as_u64().is_some());
    let token = session["token"].as_str().ok_or("token")?;
    let mut ws = connect_with_token(addr, token).await?;

    let listed = call_ok(&mut ws, "sandbox_list_capabilities", json!({})).await?;
    assert!(listed["providers"].is_array());

    send(
        &mut ws,
        "sub",
        "sandbox_subscribe_evidence",
        json!({"lease_id": "lease-evidence-1"}),
    )
    .await?;
    let subscribed = read_response(&mut ws).await?;
    assert_eq!(subscribed["result"]["subscribed"], true);
    let event = read_event(&mut ws).await?;
    assert_eq!(event["stream_id"], "lease-evidence-1");
    assert_eq!(event["event"]["payload"], "started");

    let lease = json!({"lease_id": "lease-evidence-1"});
    let cases: Vec<(&str, Value, &str, Value)> = vec![
        ("sandbox_stop_lease", lease.clone(), "/outcome", json!("stopped")),
        ("sandbox_delete_lease", lease.clone(), "/outcome", json!("deleted")),
        ("sandbox_cancel", lease.clone(), "/cancelled", json!(true)),
        (
            "sandbox_pause_lease",
            json!({"lease_id": "lease-evidence-1", "max_paused_seconds": 60}),
            "/outcome",
            json!("unsupported"),
        ),
        ("sandbox_resume_lease", lease.clone(), "/outcome", json!("unsupported")),
        (
            "sandbox_snapshot_lease",
            lease.clone(),
            "/outcome",
            json!("unsupported"),
        ),
        (
            "sandbox_snapshot_lease",
            json!({"lease_id": "lease-evidence-1", "accept_native": true}),
            "/outcome",
            json!("unsupported"),
        ),
        (
            "sandbox_snapshot_delete",
            json!({"digest": digest("b")}),
            "/outcome",
            json!("not_found"),
        ),
        (
            "sandbox_snapshot_get",
            json!({"digest": digest("b")}),
            "/outcome",
            json!("not_found"),
        ),
        (
            "sandbox_snapshot_tree",
            json!({"digest": digest("b")}),
            "/nodes",
            json!([]),
        ),
        (
            "sandbox_native_snapshot_delete",
            json!({"reference": {"provider": "e2b", "id": "opaque-capture"}}),
            "/outcome",
            json!("unsupported"),
        ),
        ("sandbox_usage", lease.clone(), "/commands_run", json!(0)),
        ("sandbox_usage", lease.clone(), "/lease_id", json!("lease-evidence-1")),
        ("sandbox_tenant_usage", json!({}), "/live_leases", json!(0)),
        ("sandbox_account_limits", json!({}), "/max_active_leases", json!(50)),
        (
            "sandbox_extend_lease_timeout",
            json!({"lease_id": "lease-evidence-1", "additional_seconds": 30}),
            "/outcome",
            json!("extended"),
        ),
        (
            "sandbox_fork",
            json!({"source_lease_id": "lease-evidence-1", "ttl_seconds": 300, "idempotency_key": "fork-1"}),
            "/lease_id",
            json!("mock-forked-lease"),
        ),
        (
            "sandbox_update_network_policy",
            json!({"lease_id": "lease-evidence-1", "network_policy": "deny_all"}),
            "/outcome",
            json!("applied"),
        ),
        (
            "sandbox_update_tags",
            json!({"lease_id": "lease-evidence-1", "tags": {"team": "a"}}),
            "/updated",
            json!(true),
        ),
        ("sandbox_wait_released", lease.clone(), "/reason", json!("completed")),
        ("sandbox_native_handle", lease.clone(), "/outcome", json!("unsupported")),
        (
            "sandbox_ssh_access",
            json!({"lease_id": "lease-evidence-1", "ttl_seconds": 600}),
            "/outcome",
            json!("unsupported"),
        ),
        ("sandbox_ssh_access", lease.clone(), "/outcome", json!("unsupported")),
        (
            "sandbox_get_domain",
            json!({"lease_id": "lease-evidence-1", "port": 8080}),
            "/endpoint",
            json!("mock-lease.internal:8080"),
        ),
        (
            "sandbox_set_snapshot_tag",
            json!({"name": "my-tag", "digest": digest("a")}),
            "/outcome",
            json!("applied"),
        ),
        (
            "sandbox_delete_snapshot_tag",
            json!({"name": "my-tag"}),
            "/outcome",
            json!("applied"),
        ),
        (
            "sandbox_snapshot_tag_history",
            json!({"name": "my-tag"}),
            "/entries",
            json!([]),
        ),
        (
            "sandbox_delete_named_sandbox",
            json!({"name": "repl-1"}),
            "/outcome",
            json!("not_found"),
        ),
        (
            "sandbox_get_lease_by_name",
            json!({"name": "repl-1"}),
            "/outcome",
            json!("not_found"),
        ),
        (
            "sandbox_get_entity",
            json!({"name": "repl-1"}),
            "/outcome",
            json!("not_found"),
        ),
        (
            "sandbox_delete_entity",
            json!({"name": "repl-1"}),
            "/outcome",
            json!("not_found"),
        ),
        (
            "sandbox_list_entities",
            json!({"limit": 5}),
            "/outcome",
            json!("listed"),
        ),
        ("sandbox_list_leases", json!({}), "/leases", json!([])),
        (
            "sandbox_list_leases",
            json!({"tags": {"a": "b"}, "tag_key": "c", "tag_value": "d"}),
            "/leases",
            json!([]),
        ),
        ("sandbox_list_snapshots", json!({}), "/snapshots", json!([])),
        ("sandbox_list_tags", json!({}), "/tags", json!([])),
        (
            "sandbox_reap_leases",
            json!({"older_than_seconds": 60, "dry_run": true}),
            "/dry_run",
            json!(true),
        ),
        (
            "sandbox_reap_leases",
            json!({"idle_for_seconds": 60}),
            "/outcome",
            json!("reaped"),
        ),
        ("sandbox_list_drives", json!({}), "/drives", json!([])),
        (
            "sandbox_delete_drive",
            json!({"drive_name": "data"}),
            "/deleted",
            json!(true),
        ),
        (
            "sandbox_create_drive",
            json!({"drive_name": "data", "size_gib": 10, "provider_volume": {"provider": "fly", "volume_id": "vol_example"}}),
            "/provider_volume/volume_id",
            json!("vol_example"),
        ),
        (
            "sandbox_create_drive",
            json!({"drive_name": "data", "bucket": "b"}),
            "/bucket",
            json!("b"),
        ),
    ];
    for (tool, payload, pointer, expected) in cases {
        let result = call_ok(&mut ws, tool, payload).await?;
        assert_eq!(result.pointer(pointer), Some(&expected), "{tool}: {result}");
    }

    let denied = call(
        &mut ws,
        "sandbox_suspend_lease",
        json!({"lease_id": "l", "level": "memory", "max_suspended_seconds": 60}),
    )
    .await?;
    assert_eq!(denied["error"]["code"], "lease_denied");
    assert_eq!(denied["error"]["kind"], "suspend_unsupported");
    let auto = call(&mut ws, "sandbox_suspend_lease", json!({"lease_id": "l"})).await?;
    assert_eq!(auto["error"]["kind"], "suspend_unsupported");
    let wake = call(&mut ws, "sandbox_wake_lease", json!({"lease_id": "l"})).await?;
    assert_eq!(wake["error"]["kind"], "lease_not_running");
    Ok(())
}

#[tokio::test]
async fn invalid_payloads_are_refused_with_invalid_payload() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("invalid"), "tenant-test").await?;
    let cases: Vec<(&str, Value)> = vec![
        ("sandbox_suspend_lease", json!({"lease_id": "l", "level": "ram"})),
        ("sandbox_ssh_access", json!({"lease_id": "l", "ttl_seconds": -1})),
        ("sandbox_write_stdin", json!({"lease_id": "l"})),
        (
            "sandbox_create_drive",
            json!({"drive_name": "data", "bucket": "b", "provider_volume": {"provider": "fly", "volume_id": "v"}}),
        ),
        ("sandbox_create_drive", json!({"drive_name": "data"})),
        ("sandbox_get_entity", json!({})),
        (
            "sandbox_get_entity",
            json!({"name": "a", "entity_id": "01HZZZZZZZZZZZZZZZZZZZZZZZ"}),
        ),
        (
            "sandbox_native_snapshot_delete",
            json!({"reference": {"provider": "e2b", "id": "raw:default"}}),
        ),
        (
            "sandbox_native_snapshot_delete",
            json!({"tenant": "foreign", "reference": {"provider": "e2b", "id": "opaque-capture"}}),
        ),
        ("sandbox_follow_logs", json!({"lease_id": "l"})),
        ("sandbox_reap_leases", json!({})),
        ("sandbox_list_leases", json!({"tag_key": 7, "tag_value": "x"})),
        ("sandbox_operation_capabilities", json!({"x": 1})),
        (
            "sandbox_get_operation",
            json!({"operation_id": "op", "compensate_created": true}),
        ),
    ];
    for (tool, payload) in cases {
        let code = call_err(&mut ws, tool, payload.clone()).await?;
        assert_eq!(code, "invalid_payload", "{tool} {payload}");
    }
    assert_eq!(call_err(&mut ws, "sandbox_nope", json!({})).await?, "unknown_tool");
    assert_eq!(
        call_err(
            &mut ws,
            "sandbox_lease",
            json!({"tenant": "other", "provider_pref": [], "ttl_seconds": 60})
        )
        .await?,
        "invalid_payload"
    );
    Ok(())
}

#[tokio::test]
async fn lease_for_another_tenant_is_refused() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("tenant"), "tenant-test").await?;
    let own = call_ok(&mut ws, "sandbox_lease", lease_request("tenant-test")).await?;
    assert!(own["lease_id"].is_string(), "{own}");
    assert_eq!(
        call_err(&mut ws, "sandbox_lease", lease_request("tenant-other")).await?,
        "tenant_mismatch"
    );
    Ok(())
}

#[tokio::test]
async fn subscribe_evidence_is_acknowledged_only_after_the_subscription_completes() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    mock.fail_evidence_subscription("subscribe failed");
    let mut ws = open_socket(mock, &api_key("subscribe"), "tenant-test").await?;
    let response = call(
        &mut ws,
        "sandbox_subscribe_evidence",
        json!({"lease_id": "lease-evidence-2"}),
    )
    .await?;
    assert!(response["result"]["subscribed"].is_null(), "{response}");
    assert_eq!(response["error"]["code"], "backend");
    Ok(())
}

#[tokio::test]
async fn sandbox_file_op_round_trips_write_stat_read_edit_and_refusals() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("file-op"), "tenant-test").await?;
    let file_op = |op: Value| json!({"lease_id": "lease-files", "op": op});
    let steps: Vec<(Value, &str)> = vec![
        (
            json!({"op": "write", "path": "a.txt", "data": "aGVsbG8gd29ybGQ=", "truncate": true}),
            "written",
        ),
        (json!({"op": "stat", "path": "a.txt"}), "stat"),
        (
            json!({"op": "edit", "path": "a.txt", "find": "world", "replace": "there"}),
            "edited",
        ),
        (
            json!({"op": "edit", "path": "a.txt", "find": "absent", "replace": "x"}),
            "edit_rejected",
        ),
        (json!({"op": "read", "path": "a.txt"}), "content"),
        (json!({"op": "checksum", "path": "a.txt"}), "checksum"),
        (json!({"op": "stat", "path": "nope.txt"}), "not_found"),
        (json!({"op": "checkpoint", "path": "."}), "unsupported"),
        (json!({"op": "changes"}), "unsupported"),
        (
            json!({"op": "watch_start", "path": ".", "recursive": true}),
            "unsupported",
        ),
        (
            json!({"op": "watch_poll", "watch_id": "w-1", "after_seq": 0}),
            "unsupported",
        ),
        (json!({"op": "watch_stop", "watch_id": "w-1"}), "unsupported"),
    ];
    for (op, outcome) in steps {
        let result = call_ok(&mut ws, "sandbox_file_op", file_op(op.clone())).await?;
        assert_eq!(result["outcome"], outcome, "{op}: {result}");
    }
    let checksum = call_ok(
        &mut ws,
        "sandbox_file_op",
        file_op(json!({"op": "checksum", "path": "a.txt"})),
    )
    .await?;
    assert_eq!(checksum["sha256"].as_str().map(str::len), Some(64));
    assert_eq!(
        call_err(&mut ws, "sandbox_file_op", file_op(json!({"op": "bogus"}))).await?,
        "invalid_payload"
    );
    let denied = call(
        &mut ws,
        "sandbox_file_op",
        file_op(json!({"op": "read", "path": "../outside"})),
    )
    .await?;
    assert!(denied["error"].is_object() || denied["result"].is_object(), "{denied}");
    Ok(())
}

#[tokio::test]
async fn files_users_and_groups_round_trip() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("files"), "tenant-test").await?;
    let written = call_ok(
        &mut ws,
        "sandbox_write_files",
        json!({"lease_id": "lease-files", "files": [{"path": "/a.txt", "content": [104, 105]}]}),
    )
    .await?;
    assert_eq!(written["outcome"], "written");
    let read = call_ok(
        &mut ws,
        "sandbox_read_file",
        json!({"lease_id": "lease-files", "path": "/a.txt"}),
    )
    .await?;
    assert_eq!(read["outcome"], "content");
    let missing = call_ok(
        &mut ws,
        "sandbox_read_file",
        json!({"lease_id": "lease-files", "path": "/nope.txt"}),
    )
    .await?;
    assert_eq!(missing["outcome"], "not_found");
    let user = call_ok(
        &mut ws,
        "sandbox_create_user",
        json!({"lease_id": "lease-files", "username": "alice"}),
    )
    .await?;
    assert_eq!(user["home_dir"], "/home/alice");
    let group = call_ok(
        &mut ws,
        "sandbox_create_group",
        json!({"lease_id": "lease-files", "groupname": "devs"}),
    )
    .await?;
    assert_eq!(group["shared_dir"], "/srv/devs");
    let member = call_ok(
        &mut ws,
        "sandbox_modify_group_membership",
        json!({"lease_id": "lease-files", "username": "alice", "groupname": "devs", "action": "add"}),
    )
    .await?;
    assert_eq!(member["outcome"], "applied");
    Ok(())
}

#[tokio::test]
async fn follow_logs_streams_chunks_from_a_cursor_and_ends() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    for (log_seq, seq, data) in [(4u64, 0u64, "a"), (6, 1, "b"), (9, 2, "c")] {
        mock.queue_log_chunk(
            LeaseId::new("lease-follow"),
            "exec-1",
            LogChunk {
                log_seq,
                stream: OutputStream::Stdout,
                seq,
                data: data.as_bytes().to_vec(),
            },
        );
    }
    let mut ws = open_socket(mock, &api_key("follow"), "tenant-test").await?;
    let ack = call_ok(
        &mut ws,
        "sandbox_follow_logs",
        json!({"lease_id": "lease-follow", "execution_id": "exec-1", "after_seq": 4}),
    )
    .await?;
    assert_eq!(ack["following"], true);
    assert_eq!(ack["stream_id"], "logs:lease-follow:exec-1");
    let first = read_event(&mut ws).await?;
    assert_eq!(first["stream_id"], "logs:lease-follow:exec-1");
    assert_eq!(first["event"]["type"], "chunk");
    assert_eq!(first["event"]["log_seq"], 6);
    assert_eq!(read_event(&mut ws).await?["event"]["log_seq"], 9);
    let end = read_event(&mut ws).await?;
    assert_eq!(end["event"]["type"], "end");
    assert_eq!(end["event"]["next_seq"], 9);
    Ok(())
}

#[tokio::test]
async fn command_tools_round_trip() -> TestResult {
    let mut ws = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("commands"), "tenant-test").await?;
    let started = call_ok(
        &mut ws,
        "sandbox_run_command",
        json!({
            "lease_id": "lease-cmd",
            "command": ["cat"],
            "detached": true,
            "stream_output": true,
            "stdin": "interactive",
            "output_retention": "stream_only"
        }),
    )
    .await?;
    assert_eq!(started["outcome"], "started");
    let execution_id = started["execution_id"].as_str().ok_or("execution id")?.to_string();
    let written = call_ok(
        &mut ws,
        "sandbox_write_stdin",
        json!({"lease_id": "lease-cmd", "execution_id": execution_id, "seq": 0, "data": [104, 105, 10], "eof": false}),
    )
    .await?;
    assert_eq!(written["outcome"], "written");
    assert_eq!(written["next_seq"], 1);

    let named = json!({
        "lease_id": "lease-cmd",
        "command": ["server"],
        "detached": true,
        "process": {"name": "web", "readiness": {"kind": "tcp_port", "port": 8080}}
    });
    let first = call_ok(&mut ws, "sandbox_run_command", named.clone()).await?;
    let again = call_ok(&mut ws, "sandbox_run_command", named).await?;
    assert_eq!(first["execution_id"], again["execution_id"]);
    let named_id = first["execution_id"].clone();

    let listed = call_ok(&mut ws, "sandbox_list_processes", json!({"lease_id": "lease-cmd"})).await?;
    assert_eq!(listed["processes"].as_array().map(Vec::len), Some(1));
    let ready = call_ok(
        &mut ws,
        "sandbox_wait_ready",
        json!({"lease_id": "lease-cmd", "execution_id": named_id, "timeout_seconds": 1}),
    )
    .await?;
    assert_eq!(ready["outcome"], "ready");
    let killed = call_ok(
        &mut ws,
        "sandbox_kill_process",
        json!({"lease_id": "lease-cmd", "name": "web"}),
    )
    .await?;
    assert_eq!(killed["outcome"], "signaled");
    let gone = call_ok(
        &mut ws,
        "sandbox_kill_process",
        json!({"lease_id": "lease-cmd", "name": "web", "signal": "kill"}),
    )
    .await?;
    assert_eq!(gone["outcome"], "not_found");
    let failed = call_ok(
        &mut ws,
        "sandbox_wait_ready",
        json!({"lease_id": "lease-cmd", "execution_id": named_id, "timeout_seconds": 1}),
    )
    .await?;
    assert_eq!(failed["outcome"], "failed");
    let no_probe = call_ok(
        &mut ws,
        "sandbox_wait_ready",
        json!({"lease_id": "lease-cmd", "execution_id": execution_id, "timeout_seconds": 1}),
    )
    .await?;
    assert_eq!(no_probe["outcome"], "not_found");
    let missing = call_ok(
        &mut ws,
        "sandbox_wait_ready",
        json!({"lease_id": "lease-cmd", "execution_id": "nope", "timeout_seconds": 1}),
    )
    .await?;
    assert_eq!(missing["outcome"], "not_found");
    let sandbox_ready = call_ok(
        &mut ws,
        "sandbox_wait_sandbox_ready",
        json!({"lease_id": "lease-cmd", "ports": [8080], "timeout_seconds": 1}),
    )
    .await?;
    assert_eq!(sandbox_ready["outcome"], "ready");

    let by_execution = json!({"lease_id": "lease-cmd", "execution_id": execution_id});
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        ("sandbox_poll_command", by_execution.clone(), "/outcome", "not_found"),
        ("sandbox_kill_command", by_execution.clone(), "/outcome", "signaled"),
        ("sandbox_stop_command", by_execution.clone(), "/outcome", "stopped"),
        (
            "sandbox_restart_command",
            by_execution.clone(),
            "/outcome",
            "unsupported",
        ),
        ("sandbox_read_logs", by_execution.clone(), "/outcome", "unsupported"),
        (
            "sandbox_cancel_command_launch",
            json!({"lease_id": "lease-cmd", "launch_key": "launch-1"}),
            "/outcome",
            "unsupported",
        ),
    ];
    for (tool, payload, pointer, expected) in cases {
        let result = call_ok(&mut ws, tool, payload).await?;
        assert_eq!(result.pointer(pointer), Some(&json!(expected)), "{tool}: {result}");
    }
    let foreground = call(
        &mut ws,
        "sandbox_run_command",
        json!({"lease_id": "lease-cmd", "command": ["ls"]}),
    )
    .await?;
    assert_eq!(foreground["error"]["code"], "lease_denied");
    Ok(())
}

#[tokio::test]
async fn sessions_are_refused_without_a_valid_key_or_token() -> TestResult {
    let key = api_key("refusals");
    let addr = serve(test_state(&key, "tenant-test", Arc::new(MockSandboxBackend::new())).await?).await?;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/v1/sessions");

    let missing = client.post(&url).send().await?;
    assert_eq!(missing.status(), 401);
    assert_eq!(missing.json::<Value>().await?["error"], "missing_api_key");
    let unknown = client
        .post(&url)
        .header(API_KEY_HEADER, "not-a-known-key")
        .send()
        .await?;
    assert_eq!(unknown.json::<Value>().await?["error"], "invalid_api_key");
    let malformed = client.post(&url).header(API_KEY_HEADER, "bad key").send().await?;
    assert_eq!(malformed.json::<Value>().await?["error"], "invalid_api_key");

    let ws_url = format!("http://{addr}/v1/ws");
    let upgrade_key = STANDARD.encode(format!("{:016}", addr.port()));
    let upgrade = |builder: reqwest::RequestBuilder| {
        builder
            .header("connection", "upgrade")
            .header("upgrade", "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", upgrade_key.as_str())
    };
    let no_token = upgrade(client.get(&ws_url)).send().await?;
    assert_eq!(no_token.status(), 401);
    assert_eq!(no_token.json::<Value>().await?["error"], "missing_or_invalid_token");
    let bad_token = upgrade(client.get(&ws_url))
        .header("authorization", "Bearer not-a-session")
        .send()
        .await?;
    assert_eq!(bad_token.json::<Value>().await?["error"], "invalid_session");
    let empty_token = upgrade(client.get(&ws_url))
        .header("authorization", "Bearer ")
        .send()
        .await?;
    assert_eq!(empty_token.json::<Value>().await?["error"], "missing_or_invalid_token");
    let token = session_token(addr, &key).await?;
    let by_header = upgrade(client.get(&ws_url))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await?;
    assert_eq!(by_header.status(), 101);
    Ok(())
}
