mod support;

use std::sync::Arc;

use futures_util::SinkExt;
use serde_json::json;
use support::{MockSandboxBackend, Release, Socket, TestResult, WAIT, api_key, open_socket, read_next, send};
use tokio_tungstenite::tungstenite::Message;

async fn send_command(socket: &mut Socket, id: &str) -> TestResult {
    send(
        socket,
        id,
        "sandbox_run_command",
        json!({"lease_id": "owned-lease", "command": ["sleep", "60"]}),
    )
    .await
}

async fn send_control(socket: &mut Socket) -> TestResult {
    send(
        socket,
        "independent-stop",
        "sandbox_kill_command",
        json!({"lease_id": "owned-lease", "execution_id": "another-owned-command", "signal": "kill"}),
    )
    .await
}

#[tokio::test]
async fn a_pending_command_does_not_block_same_socket_control() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    let (entered, release) = backend.pause_next_command();
    let _release = Release(release.clone());
    let mut socket = open_socket(backend, &api_key("control-1"), "owner").await?;
    send_command(&mut socket, "blocked-command").await?;
    tokio::time::timeout(WAIT, entered.notified()).await?;
    send_control(&mut socket).await?;
    let control = read_next(&mut socket).await?;
    assert_eq!(control["id"], "independent-stop");
    assert_eq!(control["result"]["outcome"], "signaled");
    release.notify_one();
    let command = read_next(&mut socket).await?;
    assert_eq!(command["id"], "blocked-command");
    assert_eq!(command["error"]["code"], "lease_denied");
    socket.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn duplicate_pending_request_ids_are_rejected_without_dispatching_again() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    let (entered, release) = backend.pause_next_command();
    let _release = Release(release.clone());
    let mut socket = open_socket(backend, &api_key("control-2"), "owner").await?;
    send_command(&mut socket, "same-id").await?;
    tokio::time::timeout(WAIT, entered.notified()).await?;
    send_command(&mut socket, "same-id").await?;
    let duplicate = read_next(&mut socket).await?;
    assert_eq!(duplicate["id"], "same-id");
    assert_eq!(duplicate["error"]["code"], "invalid_request_id");
    send_control(&mut socket).await?;
    assert_eq!(read_next(&mut socket).await?["id"], "independent-stop");
    release.notify_one();
    assert_eq!(read_next(&mut socket).await?["error"]["code"], "lease_denied");
    Ok(())
}

#[tokio::test]
async fn saturated_regular_requests_keep_bounded_capacity_for_control() -> TestResult {
    let backend = Arc::new(MockSandboxBackend::new());
    let mut socket = open_socket(backend.clone(), &api_key("control-3"), "owner").await?;
    let mut releases = Vec::new();
    for index in 0..24 {
        let (entered, release) = backend.pause_next_command();
        releases.push(Release(release));
        send_command(&mut socket, &format!("pending-{index}")).await?;
        tokio::time::timeout(WAIT, entered.notified()).await?;
    }
    send_command(&mut socket, "overflow").await?;
    let overflow = read_next(&mut socket).await?;
    assert_eq!(overflow["id"], "overflow");
    assert_eq!(overflow["error"]["code"], "too_many_requests");
    send_control(&mut socket).await?;
    let control = read_next(&mut socket).await?;
    assert_eq!(control["id"], "independent-stop");
    assert_eq!(control["result"]["outcome"], "signaled");
    drop(releases);
    let mut completed = std::collections::HashSet::new();
    for _ in 0..24 {
        let response = read_next(&mut socket).await?;
        assert_eq!(response["error"]["code"], "lease_denied");
        let id = response["id"].as_str().ok_or("response id")?.to_owned();
        assert!(completed.insert(id));
    }
    Ok(())
}

#[tokio::test]
async fn malformed_frames_and_ids_are_refused_and_the_socket_survives() -> TestResult {
    let mut socket = open_socket(Arc::new(MockSandboxBackend::new()), &api_key("control-4"), "owner").await?;
    socket.send(Message::Text("not json".into())).await?;
    let invalid = read_next(&mut socket).await?;
    assert_eq!(invalid["error"]["code"], "invalid_frame", "{invalid}");
    socket.send(Message::Binary(vec![1, 2, 3].into())).await?;
    send(&mut socket, "", "sandbox_list_capabilities", json!({})).await?;
    assert_eq!(read_next(&mut socket).await?["error"]["code"], "invalid_request_id");
    let long_id = "x".repeat(200);
    send(&mut socket, &long_id, "sandbox_list_capabilities", json!({})).await?;
    assert_eq!(read_next(&mut socket).await?["error"]["code"], "invalid_request_id");
    send(&mut socket, "ok", "sandbox_list_capabilities", json!({})).await?;
    assert!(read_next(&mut socket).await?["error"].is_null());
    socket.send(Message::Ping(vec![1].into())).await?;
    socket.send(Message::Pong(vec![1].into())).await?;
    send(&mut socket, "after-ping", "sandbox_list_capabilities", json!({})).await?;
    assert_eq!(read_next(&mut socket).await?["id"], "after-ping");
    socket.close(None).await?;
    Ok(())
}
