mod support;

use std::time::Duration;

use serde_json::json;
use support::{FakeTransport, TestResult, doc, server_for, server_with};
use tokio::time::Instant;
use trogon_sandbox_lsp::{ExitState, LspError, LspServerOptions, Signal};

fn down() -> Result<ExitState, LspError> {
    Err(LspError::Transport("down".into()))
}

#[tokio::test(start_paused = true)]
async fn stop_runs_shutdown_exit_eof_and_returns_once_the_server_exits() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    server.stop().await;

    let methods = transport.methods();
    assert_eq!(&methods[2..], &["shutdown", "exit"]);
    assert!(transport.eof_seen());
    assert!(transport.kills().is_empty());
    let err = server
        .did_open(&doc("a.ts")?, "", None)
        .await
        .err()
        .ok_or("open succeeded")?;
    assert!(matches!(err, LspError::NotRunning(_)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stop_escalates_to_term_then_kill() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_with(&transport, LspServerOptions::default())?;
    server.start().await?;
    let started = Instant::now();
    server.stop().await;
    assert_eq!(transport.kills(), vec![Signal::Term, Signal::Kill]);
    assert_eq!(started.elapsed(), Duration::from_secs(5) + Duration::from_secs(2));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stop_skips_kill_when_term_is_enough() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    transport.exit_on_signal(Signal::Term);
    let (server, _) = server_with(&transport, LspServerOptions::default())?;
    server.start().await?;
    server.stop().await;
    assert_eq!(transport.kills(), vec![Signal::Term]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stop_gives_up_waiting_when_exit_polling_fails() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    transport.script_exit_polls(vec![Ok(ExitState::Running), down(), down()]);
    let (server, _) = server_with(&transport, LspServerOptions::default())?;
    server.start().await?;
    server.stop().await;
    assert_eq!(transport.kills(), vec![Signal::Term, Signal::Kill]);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn stop_is_idempotent_even_when_concurrent() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    tokio::join!(server.stop(), server.stop());
    server.stop().await;
    let methods = transport.methods();
    let shutdowns = methods.iter().filter(|m| *m == "shutdown").count();
    let exits = methods.iter().filter(|m| *m == "exit").count();
    assert_eq!((shutdowns, exits), (1, 1));
    Ok(())
}

#[tokio::test]
async fn stop_before_start_is_a_no_op_and_blocks_later_start() -> TestResult {
    let transport = FakeTransport::new();
    let (server, launcher) = server_for(&transport)?;
    server.stop().await;
    let err = server.start().await.err().ok_or("start succeeded")?;
    assert!(matches!(err, LspError::NotRunning(_)));
    assert!(launcher.launched().is_empty());
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exit_watch_reports_a_server_exit_to_pending_requests() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    transport.exit_with(Some(3));
    let err = server
        .request("slow", None, None)
        .await
        .err()
        .ok_or("request succeeded")?;
    assert!(matches!(err, LspError::ServerExited(Some(3))));
    assert_eq!(err.to_string(), "language server exited with code 3");
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exit_watch_tolerates_four_poll_failures() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let mut script: Vec<Result<ExitState, LspError>> = (0..4).map(|_| down()).collect();
    script.push(Ok(ExitState::Running));
    transport.script_exit_polls(script);
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(transport.polls() >= 5);
    server.notify("ping", None).await?;
    server.stop().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn exit_watch_gives_up_after_five_consecutive_failures() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    transport.script_exit_polls((0..5).map(|_| down()).collect());
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let err = server
        .request("slow", None, None)
        .await
        .err()
        .ok_or("request succeeded")?;
    assert!(matches!(&err, LspError::NotRunning(m) if m.contains("exit polling failed 5 times")));
    let later = server.notify("ping", None).await.err().ok_or("notify succeeded")?;
    assert!(matches!(later, LspError::NotRunning(_)));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_success_resets_the_poll_failure_count() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let mut script: Vec<Result<ExitState, LspError>> = Vec::new();
    for _ in 0..3 {
        script.extend((0..4).map(|_| down()));
        script.push(Ok(ExitState::Running));
    }
    transport.script_exit_polls(script);
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    tokio::time::sleep(Duration::from_secs(16)).await;
    server.notify("ping", None).await?;
    server.stop().await;
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_server_that_exited_before_the_watch_fired_is_not_signalled() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    transport.exit_with(Some(0));
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    server.stop().await;
    assert!(transport.kills().is_empty());
    assert_eq!(json!(transport.methods().len()), json!(2));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn an_output_error_after_the_server_exited_does_not_kill_again() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    transport.exit_with(Some(1));
    let first = server
        .request("slow", None, None)
        .await
        .err()
        .ok_or("request succeeded")?;
    assert!(matches!(first, LspError::ServerExited(Some(1))));
    transport.emit_error(LspError::Transport("late".into()));
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(transport.kills().is_empty());
    Ok(())
}
