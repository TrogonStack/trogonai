mod support;

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use support::{MockSandboxBackend, Socket, TerminalFault, TestResult, api_key, call, call_ok, open_socket, read_event};
use trogon_sandbox_contract::{TerminalDimension, TerminalOp};

async fn open_terminal(socket: &mut Socket, lease: &str) -> TestResult<String> {
    let opened = call_ok(
        socket,
        "sandbox_terminal_open",
        json!({"lease_id": lease, "cols": 80, "rows": 24}),
    )
    .await?;
    assert_eq!(opened["outcome"], "opened", "{opened}");
    Ok(opened["terminal_id"].as_str().ok_or("terminal id")?.to_string())
}

#[tokio::test]
async fn open_streams_output_and_forwards_input_resize_and_close() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    let mut ws = open_socket(mock.clone(), &api_key("term-1"), "tenant-test").await?;

    let terminal = open_terminal(&mut ws, "lease-term").await?;
    let event = read_event(&mut ws).await?;
    assert_eq!(event["stream_id"], terminal.as_str());
    assert_eq!(event["event"]["type"], "data");
    assert_eq!(mock.terminal_subscriptions(), vec![terminal.clone()]);

    for (tool, payload) in [
        (
            "sandbox_terminal_input",
            json!({"terminal_id": terminal, "data": "bHMK"}),
        ),
        (
            "sandbox_terminal_resize",
            json!({"terminal_id": terminal, "cols": 120, "rows": 40}),
        ),
        ("sandbox_terminal_close", json!({"terminal_id": terminal})),
    ] {
        let result = call_ok(&mut ws, tool, payload).await?;
        assert_eq!(result["outcome"], "done", "{tool}: {result}");
    }

    let ops: Vec<TerminalOp> = mock.terminal_ops().into_iter().map(|(_, _, op)| op).collect();
    assert_eq!(ops.len(), 4);
    assert!(matches!(ops[0], TerminalOp::Open { .. }));
    assert!(matches!(ops[1], TerminalOp::Input { .. }));
    assert_eq!(
        ops[2],
        TerminalOp::Resize {
            cols: TerminalDimension::new(120)?,
            rows: TerminalDimension::new(40)?
        }
    );
    assert_eq!(ops[3], TerminalOp::Close);

    let late = call(
        &mut ws,
        "sandbox_terminal_input",
        json!({"terminal_id": terminal, "data": "eA=="}),
    )
    .await?;
    assert_eq!(late["error"]["code"], "terminal_failed", "{late}");
    assert_eq!(late["error"]["kind"], "not_found", "{late}");
    assert_eq!(mock.terminal_ops().len(), 4);
    Ok(())
}

#[tokio::test]
async fn a_socket_cannot_drive_or_close_another_sockets_terminal() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    let key = api_key("term-2");
    let addr = support::serve(support::test_state(&key, "tenant-test", mock.clone()).await?).await?;
    let mut owner = support::connect_with_token(addr, &support::session_token(addr, &key).await?).await?;
    let mut intruder = support::connect_with_token(addr, &support::session_token(addr, &key).await?).await?;

    let terminal = open_terminal(&mut owner, "lease-owner").await?;
    for (tool, payload) in [
        (
            "sandbox_terminal_input",
            json!({"terminal_id": terminal, "data": "eA=="}),
        ),
        (
            "sandbox_terminal_resize",
            json!({"terminal_id": terminal, "cols": 10, "rows": 10}),
        ),
    ] {
        let response = call(&mut intruder, tool, payload).await?;
        assert_eq!(response["error"]["kind"], "not_found", "{tool}: {response}");
    }
    let close = call_ok(
        &mut intruder,
        "sandbox_terminal_close",
        json!({"terminal_id": terminal}),
    )
    .await?;
    assert_eq!(close["outcome"], "done");
    assert_eq!(mock.terminal_ops().len(), 1);
    let still = call_ok(
        &mut owner,
        "sandbox_terminal_input",
        json!({"terminal_id": terminal, "data": "eA=="}),
    )
    .await?;
    assert_eq!(still["outcome"], "done");
    Ok(())
}

#[tokio::test]
async fn open_on_a_provider_without_pty_is_a_typed_unsupported_error() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    mock.set_terminal_unsupported(true);
    let mut ws = open_socket(mock, &api_key("term-3"), "tenant-test").await?;
    let response = call(
        &mut ws,
        "sandbox_terminal_open",
        json!({"lease_id": "lease-x", "cols": 80, "rows": 24}),
    )
    .await?;
    assert_eq!(response["error"]["code"], "unsupported", "{response}");
    assert_eq!(response["error"]["retryable"], false);
    Ok(())
}

#[tokio::test]
async fn bad_dimensions_and_ids_are_refused_before_the_backend() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    let mut ws = open_socket(mock.clone(), &api_key("term-4"), "tenant-test").await?;
    for payload in [
        json!({"lease_id": "lease-x", "cols": 0, "rows": 24}),
        json!({"lease_id": "lease-x", "cols": 80}),
    ] {
        let response = call(&mut ws, "sandbox_terminal_open", payload).await?;
        assert_eq!(response["error"]["code"], "invalid_payload", "{response}");
    }
    let empty_id = call(&mut ws, "sandbox_terminal_close", json!({"terminal_id": ""})).await?;
    assert_eq!(empty_id["error"]["code"], "invalid_payload");
    assert!(mock.terminal_ops().is_empty());
    assert!(mock.terminal_subscriptions().is_empty());
    Ok(())
}

#[tokio::test]
async fn dropping_the_socket_closes_its_open_terminals() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    let mut ws = open_socket(mock.clone(), &api_key("term-5"), "tenant-test").await?;
    let terminal = open_terminal(&mut ws, "lease-drop").await?;
    drop(ws);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let closed = mock
                .terminal_ops()
                .iter()
                .any(|(_, id, op)| id.as_str() == terminal && *op == TerminalOp::Close);
            if closed {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn open_failures_map_to_typed_errors_and_clean_up() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    let mut ws = open_socket(mock.clone(), &api_key("term-6"), "tenant-test").await?;
    let open = json!({"lease_id": "lease-x", "cols": 80, "rows": 24, "cmd": ["sh"], "env": {"A": "b"}, "cwd": "/tmp"});

    mock.set_terminal_fault(Some(TerminalFault::Failed));
    let failed = call(&mut ws, "sandbox_terminal_open", open.clone()).await?;
    assert_eq!(failed["error"]["code"], "terminal_failed", "{failed}");

    mock.set_terminal_fault(Some(TerminalFault::Denied));
    let denied = call(&mut ws, "sandbox_terminal_open", open.clone()).await?;
    assert_eq!(denied["error"]["code"], "lease_denied", "{denied}");

    mock.set_terminal_fault(Some(TerminalFault::Timeout));
    let before = mock.terminal_ops().len();
    let timeout = call(&mut ws, "sandbox_terminal_open", open.clone()).await?;
    assert!(timeout["error"].is_object(), "{timeout}");
    assert_eq!(mock.terminal_ops().len(), before + 2, "the open and a cleanup close");

    mock.set_terminal_fault(None);
    mock.fail_terminal_subscription("no subscription");
    let subscribe = call(&mut ws, "sandbox_terminal_open", open).await?;
    assert_eq!(subscribe["error"]["code"], "backend", "{subscribe}");
    Ok(())
}

#[tokio::test]
async fn a_socket_holds_a_bounded_number_of_terminals() -> TestResult {
    let mock = Arc::new(MockSandboxBackend::new());
    let mut ws = open_socket(mock, &api_key("term-7"), "tenant-test").await?;
    let mut last: Value = Value::Null;
    for _ in 0..33 {
        last = call(
            &mut ws,
            "sandbox_terminal_open",
            json!({"lease_id": "lease-many", "cols": 80, "rows": 24}),
        )
        .await?;
    }
    assert_eq!(last["error"]["code"], "terminal_failed", "{last}");
    assert_eq!(last["error"]["kind"], "too_many_terminals", "{last}");
    Ok(())
}
