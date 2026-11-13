#![cfg(feature = "test-support")]

use std::{
    error::Error as StdError,
    time::{Duration, Instant},
};

use tokio::time::timeout;
use trogon_claude_agent_sdk::{
    ClaudeAgentOptions, ClaudeSdkClient, DuplicateControlRequests, Error, InitializeTimeout, InputAfterFinish, Limits,
    MessageBuffer, MirrorFailureError, Prompt, QueueCapacity, RequestIdExhaustion, SessionEntry, SessionKey,
    SessionStore, SessionStoreError, SessionStoreFuture, SessionStoreLimits, SessionStoreOptions, Shutdown, Timeout,
    TranscriptMirror,
};

type TestResult = Result<(), Box<dyn StdError>>;

fn options(mode: &str) -> Result<ClaudeAgentOptions, Box<dyn StdError>> {
    let mut options = ClaudeAgentOptions {
        cli_path: Some(std::path::absolute(env!("CARGO_BIN_EXE_claude-sdk-fixture"))?),
        model: Some(format!("fixture:{mode}").into()),
        ..ClaudeAgentOptions::default()
    };
    options.env.insert("LLVM_PROFILE_FILE".into(), "/dev/null".into());
    Ok(options)
}

async fn first_error(client: &mut ClaudeSdkClient) -> Result<Option<Error>, Box<dyn StdError>> {
    Ok(timeout(Duration::from_secs(10), async {
        loop {
            match client.next_message().await {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await?)
}

#[test]
fn the_default_limits_are_the_typescript_sdk() {
    let limits = Limits::default();
    assert_eq!(limits.max_buffer_size, None);
    assert_eq!(limits.stderr_tail, 2048);
    assert_eq!(limits.message_buffer, MessageBuffer::Unbounded);
    assert_eq!(limits.command_queue_capacity, None);
    assert_eq!(limits.command_queue_timeout, None);
    assert_eq!(limits.max_pending_controls, None);
    assert_eq!(limits.max_incoming_callbacks, None);
    assert_eq!(limits.max_tracked_tasks, None);
    assert_eq!(limits.max_queued_inputs, None);
    assert_eq!(limits.control_timeout, None);
    assert_eq!(limits.initialize_timeout, InitializeTimeout::TypeScript);
    assert_eq!(limits.callback_timeout, None);
    assert_eq!(limits.write_timeout, None);
    assert_eq!(limits.shutdown, Shutdown::TypeScript);
    assert_eq!(limits.run_end_timeout.as_duration(), Duration::from_millis(600_000));
    assert_eq!(limits.duplicate_control_requests, DuplicateControlRequests::Skip);
    assert_eq!(limits.request_id_exhaustion, RequestIdExhaustion::Wrap);
    assert_eq!(limits.input_after_finish, InputAfterFinish::Write);
}

#[test]
fn the_python_preset_selects_the_python_sdk_values() {
    let limits = Limits::python();
    assert_eq!(limits.max_buffer_size.map(|size| size.get()), Some(1024 * 1024));
    assert_eq!(limits.stderr_tail, 0);
    assert_eq!(
        limits.message_buffer,
        MessageBuffer::Backpressure(QueueCapacity::PYTHON_MESSAGES)
    );
    assert_eq!(QueueCapacity::PYTHON_MESSAGES.get(), 100);
    assert_eq!(limits.control_timeout, Some(Timeout::new(Duration::from_secs(60))));
    assert_eq!(limits.initialize_timeout, InitializeTimeout::Python);
    assert_eq!(limits.shutdown, Shutdown::Python);
    assert_eq!(limits.duplicate_control_requests, DuplicateControlRequests::RunBoth);
    assert_eq!(limits.write_timeout, None);
    assert_eq!(limits.callback_timeout, None);
    assert_eq!(limits.max_incoming_callbacks, None);
}

#[tokio::test]
async fn m08_a_long_stdout_line_is_not_capped_by_default() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("oversized")?).await?;
    client.send_prompt(Prompt::from("oversized")).await?;
    let outcome = timeout(Duration::from_secs(2), async {
        loop {
            match client.next_message().await {
                Ok(Some(_)) => {}
                Ok(None) => return None,
                Err(error) => return Some(error),
            }
        }
    })
    .await;
    if let Ok(error) = outcome {
        assert!(error.is_none(), "{error:?}");
    }
    let _ = timeout(Duration::from_secs(15), client.disconnect()).await;
    Ok(())
}

#[tokio::test]
async fn r08_a_process_error_keeps_the_last_2048_stderr_units_trimmed() -> TestResult {
    let mut client = ClaudeSdkClient::connect(options("failure")?).await?;
    client.send_prompt(Prompt::from("failure")).await?;
    let Some(Error::Process { status, stderr }) = first_error(&mut client).await? else {
        return Err(std::io::Error::other("expected a process error").into());
    };
    assert_eq!(status.code(), Some(7));
    assert_eq!(stderr.as_bytes().len(), 2048);
    assert!(stderr.as_bytes().ends_with(b"diagnostic-tail"));
    let _ = client.disconnect().await;
    Ok(())
}

#[tokio::test]
async fn m06_a_control_request_has_no_deadline_by_default() -> TestResult {
    let client = ClaudeSdkClient::connect(options("control-timeout")?).await?;
    let handle = client.handle();
    let pending = tokio::spawn(async move { handle.interrupt().await });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!pending.is_finished());
    pending.abort();
    let _ = timeout(Duration::from_secs(15), client.disconnect()).await?;
    Ok(())
}

#[tokio::test]
async fn r07_the_default_shutdown_stops_a_stuck_cli_without_an_error() -> TestResult {
    let client = ClaudeSdkClient::connect(options("blocked-stdin")?).await?;
    client.send_prompt(Prompt::from("block")).await?;
    let started = Instant::now();
    let outcome = timeout(Duration::from_secs(10), client.disconnect()).await?;
    assert!(outcome.is_ok(), "{outcome:?}");
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert!(started.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[test]
fn the_default_store_limits_bound_only_what_upstream_bounds() {
    let limits = SessionStoreLimits::default();
    assert_eq!(limits.max_retained_failures, None);
    assert_eq!(limits.flush_timeout, None);
    assert_eq!(limits.append_timeout.as_duration(), Duration::from_secs(60));
    assert_eq!(limits.load_timeout.as_duration(), Duration::from_secs(60));
    assert_eq!(limits.max_pending_entries.get(), 500);
    assert_eq!(limits.max_pending_bytes.get(), 1024 * 1024);
}

const MIRROR_PROJECTS: &str = "/memory-config/projects";

fn mirror_entry() -> Result<SessionEntry, Box<dyn StdError>> {
    Ok(SessionEntry::try_from(serde_json::from_str::<serde_json::Value>(
        r#"{"type":"user","uuid":"entry"}"#,
    )?)?)
}

fn mirror_path(session: &str) -> std::path::PathBuf {
    std::path::Path::new(MIRROR_PROJECTS)
        .join("project")
        .join(format!("{session}.jsonl"))
}

/// Fails every append, like an unreachable backend.
struct RejectingStore;

impl SessionStore for RejectingStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async { Err(SessionStoreError::backend(std::io::Error::other("fixture failure"))) })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn the_mirror_reports_every_failed_append_by_default() -> TestResult {
    let options = SessionStoreOptions::from(std::sync::Arc::new(RejectingStore));
    let mirror = TranscriptMirror::new(&options, MIRROR_PROJECTS.into());
    let sessions = [
        "123e4567-e89b-42d3-a456-426614174000",
        "123e4567-e89b-42d3-a456-426614174001",
        "123e4567-e89b-42d3-a456-426614174002",
    ];
    for session in sessions {
        mirror.enqueue(&mirror_path(session), vec![mirror_entry()?])?;
    }
    let failures = mirror.flush().await?;
    assert_eq!(failures.len(), sessions.len());
    assert!(
        failures
            .iter()
            .all(|failure| matches!(failure, MirrorFailureError::Append { .. }))
    );
    Ok(())
}

/// Accepts every append after a delay longer than any flush deadline Rust used to impose.
struct SlowStore;

impl SessionStore for SlowStore {
    fn append<'a>(&'a self, _: &'a SessionKey, _: &'a [SessionEntry]) -> SessionStoreFuture<'a, ()> {
        Box::pin(async {
            tokio::time::sleep(Duration::from_secs(600)).await;
            Ok(())
        })
    }
    fn load<'a>(&'a self, _: &'a SessionKey) -> SessionStoreFuture<'a, Option<Vec<SessionEntry>>> {
        Box::pin(async { Ok(None) })
    }
}

#[tokio::test(start_paused = true)]
async fn a_mirror_flush_waits_for_slow_appends_without_a_deadline_by_default() -> TestResult {
    let mut options = SessionStoreOptions::from(std::sync::Arc::new(SlowStore));
    options.limits.append_timeout = Timeout::new(Duration::from_secs(3600));
    let mirror = TranscriptMirror::new(&options, MIRROR_PROJECTS.into());
    mirror.enqueue(
        &mirror_path("123e4567-e89b-42d3-a456-426614174000"),
        vec![mirror_entry()?],
    )?;
    let started = tokio::time::Instant::now();
    assert!(mirror.flush().await?.is_empty());
    assert!(started.elapsed() >= Duration::from_secs(600));
    Ok(())
}
