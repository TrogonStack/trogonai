mod command_support;
mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use command_support::{
    CMD, Reply, Server, TestResult, error_reply, exec_within, exit_record, kill_request, run_request, sandbox, started,
};
use serde_json::{Value, json};
use trogon_sandbox_contract::{
    KillCommandResponse, PollCommandResponse, RunCommandRequest, RunCommandResponse, UnixSignal,
};
use trogon_sandbox_vercel_worker::VercelExec;

const LEASE: &str = "surface-lease";

fn request(extra: Value) -> Result<RunCommandRequest, serde_json::Error> {
    run_request(LEASE, json!(["true"]), extra)
}

#[tokio::test]
async fn failed_launches_and_streams_without_an_exit_are_denied() -> TestResult {
    let calls = AtomicUsize::new(0);
    let server = Server::start(move |_| match calls.fetch_add(1, Ordering::SeqCst) {
        0 => error_reply(500, "launch refused"),
        1 => Reply::Complete(
            200,
            b"{\"stream\":\"error\",\"data\":{\"message\":\"broken stream\"}}\n".to_vec(),
        ),
        _ => Reply::Complete(200, Vec::new()),
    })
    .await?;
    let exec = server.exec()?;
    let sandbox = sandbox();
    let detached = exec
        .handle_run_command(&sandbox, &request(json!({"detached": true}))?)
        .await;
    assert!(matches!(detached, RunCommandResponse::Denied(_)));
    for _ in 0..2 {
        let response = exec.handle_run_command(&sandbox, &request(json!({}))?).await;
        assert!(matches!(response, RunCommandResponse::Denied(_)));
    }
    server.shutdown().await;
    Ok(())
}

async fn start(exec: &VercelExec) -> TestResult {
    started(
        exec.handle_run_command(&sandbox(), &request(json!({"detached": true}))?)
            .await,
    )?;
    Ok(())
}

#[tokio::test]
async fn kill_and_poll_report_native_status_outcomes() -> TestResult {
    let status = AtomicUsize::new(0);
    let kills = AtomicUsize::new(0);
    let server = Server::start(move |request| {
        if request.route().ends_with("/kill") {
            return error_reply(
                if kills.fetch_add(1, Ordering::SeqCst) == 0 {
                    404
                } else {
                    500
                },
                "kill refused",
            );
        }
        if request.method == "POST" && request.route() == CMD {
            return Reply::json(200, exit_record("native-command", None::<i32>));
        }
        match status.fetch_add(1, Ordering::SeqCst) {
            0 => Reply::json(200, exit_record("native-command", 3)),
            1 => error_reply(404, "gone"),
            2 => error_reply(500, "status refused"),
            3 | 4 => Reply::json(200, exit_record("native-command", None::<i32>)),
            5 => Reply::Silent,
            _ => Reply::Held(b"{".to_vec()),
        }
    })
    .await?;
    let exec = server.exec()?;
    let sandbox = sandbox();
    let unknown = kill_request(LEASE, "unknown", UnixSignal::Term);
    assert_eq!(
        exec.handle_kill_command(&sandbox, LEASE, &unknown).await,
        KillCommandResponse::NotFound
    );
    let kill = kill_request(LEASE, "native-command", UnixSignal::Hup);
    start(&exec).await?;
    assert_eq!(
        exec.handle_kill_command(&sandbox, LEASE, &kill).await,
        KillCommandResponse::AlreadyCompleted
    );
    start(&exec).await?;
    assert_eq!(
        exec.handle_kill_command(&sandbox, LEASE, &kill).await,
        KillCommandResponse::NotFound
    );
    start(&exec).await?;
    assert!(matches!(
        exec.handle_kill_command(&sandbox, LEASE, &kill).await,
        KillCommandResponse::Denied(_)
    ));
    start(&exec).await?;
    assert_eq!(
        exec.handle_kill_command(&sandbox, LEASE, &kill).await,
        KillCommandResponse::NotFound
    );
    start(&exec).await?;
    assert!(matches!(
        exec.handle_kill_command(&sandbox, LEASE, &kill).await,
        KillCommandResponse::Denied(_)
    ));
    for _ in 0..2 {
        start(&exec).await?;
        let polled = exec.handle_poll_command(&sandbox, LEASE, "native-command").await;
        assert!(matches!(polled, PollCommandResponse::Running));
    }
    for _ in 0..2 {
        start(&exec).await?;
        let polled = exec.handle_poll_command(&sandbox, LEASE, "native-command").await;
        assert!(matches!(polled, PollCommandResponse::Running));
    }
    start(&exec).await?;
    server.shutdown().await;
    let unreachable = exec.handle_poll_command(&sandbox, LEASE, "native-command").await;
    assert!(matches!(unreachable, PollCommandResponse::Denied(_)));
    Ok(())
}

#[tokio::test]
async fn a_recovery_that_outlives_the_log_grace_is_denied_but_retryable() -> TestResult {
    let server = Server::start(|request| {
        if request.method == "POST" && request.route() == CMD {
            return match request.json() {
                Ok(body) if command_support::is_control(&body) => Reply::Silent,
                _ => Reply::json(200, exit_record("native-command", None::<i32>)),
            };
        }
        if request.route().ends_with("/logs") {
            return Reply::Held(Vec::new());
        }
        Reply::json(200, exit_record("native-command", 0))
    })
    .await?;
    let exec = exec_within(&server, 60)?;
    start(&exec).await?;
    let polled = tokio::time::timeout(
        Duration::from_secs(50),
        exec.handle_poll_command(&sandbox(), LEASE, "native-command"),
    )
    .await?;
    server.shutdown().await;
    assert!(matches!(polled, PollCommandResponse::Denied(_)));
    Ok(())
}
