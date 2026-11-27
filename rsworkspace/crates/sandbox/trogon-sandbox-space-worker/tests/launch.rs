mod nats_common;
mod support;

use std::path::Path;
use std::time::Duration;

use clap::Parser;
use support::{MANAGEMENT_TOKEN, Space, TestResult, closed_origin};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_protocol::AuthorityMode;
use trogon_sandbox_space_worker::{LaunchError, WorkerArgs, launch_until};

fn args(space: &str, nats: &str, state: &Path, extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let state = state.to_string_lossy().into_owned();
    let mut line = vec![
        "worker",
        "--endpoint",
        space,
        "--data-plane-url",
        space,
        "--token",
        MANAGEMENT_TOKEN,
        "--nats-url",
        nats,
        "--state-dir",
        &state,
    ];
    line.extend_from_slice(extra);
    WorkerArgs::try_parse_from(line)
}

async fn run(args: WorkerArgs) -> Result<(), LaunchError> {
    launch_until(args, AuthorityMode::Off, CancellationToken::new()).await
}

#[tokio::test]
async fn refuses_settings_that_do_not_parse() -> TestResult {
    let space = Space::start().await;
    let uri = space.uri();
    let dir = tempfile::tempdir()?;
    let nowhere = "nats://127.0.0.1:1";

    let remote = args("http://example.com", nowhere, dir.path(), &[])?;
    assert!(matches!(run(remote).await, Err(LaunchError::Origin(_))));

    let mut empty = args(&uri, nowhere, dir.path(), &[])?;
    empty.token = String::new();
    assert!(matches!(run(empty).await, Err(LaunchError::Token(_))));

    let template = args(&uri, nowhere, dir.path(), &["--template", "bad template"])?;
    assert!(matches!(run(template).await, Err(LaunchError::Template(_))));

    let leases = args(&uri, nowhere, dir.path(), &["--max-leases", "0"])?;
    assert!(matches!(run(leases).await, Err(LaunchError::MaxLeases(_))));

    let boot = args(&uri, nowhere, dir.path(), &["--boot-timeout-seconds", "0"])?;
    assert!(matches!(run(boot).await, Err(LaunchError::Config(_))));
    Ok(())
}

#[tokio::test]
async fn refuses_a_state_directory_it_cannot_open() -> TestResult {
    let space = Space::start().await;
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("occupied");
    std::fs::write(&file, b"x")?;
    let state = file.join("below");
    let settings = args(&space.uri(), "nats://127.0.0.1:1", &state, &[])?;
    assert!(matches!(run(settings).await, Err(LaunchError::Open(_))));
    Ok(())
}

#[tokio::test]
async fn refuses_to_start_when_reclaiming_fails() -> TestResult {
    let space = Space::start().await;
    space.state().list_status = Some(500);
    let dir = tempfile::tempdir()?;
    let settings = args(&space.uri(), "nats://127.0.0.1:1", dir.path(), &[])?;
    assert!(matches!(run(settings).await, Err(LaunchError::Recover(_))));
    Ok(())
}

#[tokio::test]
async fn refuses_a_worker_id_that_is_not_the_durable_one() -> TestResult {
    let space = Space::start().await;
    let dir = tempfile::tempdir()?;
    let settings = args(
        &space.uri(),
        "nats://127.0.0.1:1",
        dir.path(),
        &["--worker-id", "someone-else"],
    )?;
    assert!(matches!(run(settings).await, Err(LaunchError::WorkerIdMismatch)));
    Ok(())
}

#[tokio::test]
async fn reports_a_nats_server_that_is_not_there() -> TestResult {
    let space = Space::start().await;
    let dir = tempfile::tempdir()?;
    let closed = closed_origin().await?.replace("http://", "nats://");
    let settings = args(&space.uri(), &closed, dir.path(), &[])?;
    assert!(matches!(run(settings).await, Err(LaunchError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn serves_until_told_to_stop() -> TestResult {
    let space = Space::start().await;
    let server = nats_common::Server::start().await;
    let dir = tempfile::tempdir()?;
    let settings = args(&space.uri(), &server.url(), dir.path(), &[])?;
    let stop = CancellationToken::new();
    let task = tokio::spawn(launch_until(settings, AuthorityMode::Off, stop.clone()));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!task.is_finished());
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), task).await???;
    Ok(())
}
