mod support;

use std::time::Duration;

use clap::Parser;
use support::{FakeHypervisor, TestResult};
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_hyperlight_worker::{
    LaunchError, RuntimeError, UnsupportedHypervisor, WorkerArgs, launch, launch_until, launch_with,
};
use trogon_sandbox_protocol::AuthorityMode;

fn args(root: &std::path::Path, nats: &str, extra: &[&str]) -> Result<WorkerArgs, clap::Error> {
    let root = root.to_string_lossy().into_owned();
    let mut line = vec!["worker", "--component-root", &root, "--nats-url", nats];
    line.extend_from_slice(extra);
    WorkerArgs::try_parse_from(line)
}

async fn run(args: WorkerArgs) -> Result<(), LaunchError> {
    launch_until(
        args,
        AuthorityMode::Off,
        FakeHypervisor::default(),
        CancellationToken::new(),
    )
    .await
}

#[tokio::test]
async fn refuses_settings_that_do_not_hold_up() -> TestResult {
    let dir = tempfile::tempdir()?;
    let nowhere = "nats://127.0.0.1:1";

    let registry = args(dir.path(), nowhere, &["--insecure-registries", "not a host"])?;
    assert!(matches!(run(registry).await, Err(LaunchError::Registry(_))));

    let root = args(&dir.path().join("missing"), nowhere, &[])?;
    assert!(matches!(
        run(root).await,
        Err(LaunchError::Runtime(RuntimeError::ComponentRoot(_)))
    ));

    let bare = args(dir.path(), nowhere, &[])?;
    let hypervisor = launch_until(
        bare,
        AuthorityMode::Off,
        UnsupportedHypervisor,
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(
        hypervisor,
        Err(LaunchError::Runtime(RuntimeError::NoHypervisor))
    ));

    let unreachable = args(dir.path(), nowhere, &[])?;
    assert!(matches!(run(unreachable).await, Err(LaunchError::Connect(_))));
    Ok(())
}

#[tokio::test]
async fn serves_until_told_to_stop() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let dir = tempfile::tempdir()?;
    let url = format!("nats://{}", server.address());
    let settings = args(dir.path(), &url, &["--insecure-registries", "127.0.0.1:5000,"])?;
    let stop = CancellationToken::new();
    let task = tokio::spawn(launch_until(
        settings,
        AuthorityMode::Off,
        FakeHypervisor::default(),
        stop.clone(),
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!task.is_finished());
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(10), task).await???;
    Ok(())
}

#[tokio::test]
async fn stops_when_interrupted() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let dir = tempfile::tempdir()?;
    let url = format!("nats://{}", server.address());
    let (interrupt, interrupted) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(launch_with(
        args(dir.path(), &url, &[])?,
        FakeHypervisor::default(),
        async move {
            let _ = interrupted.await;
        },
    ));
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!task.is_finished());
    assert!(interrupt.send(()).is_ok());
    tokio::time::timeout(Duration::from_secs(10), task).await???;
    Ok(())
}

#[tokio::test]
async fn an_interrupt_before_the_start_still_reports_the_refusal() -> TestResult {
    let dir = tempfile::tempdir()?;
    let settings = args(&dir.path().join("missing"), "nats://127.0.0.1:1", &[])?;
    let result = launch_with(settings, FakeHypervisor::default(), async {}).await;
    assert!(matches!(result, Err(LaunchError::Runtime(_))));
    let settings = args(&dir.path().join("missing"), "nats://127.0.0.1:1", &[])?;
    assert!(matches!(
        launch(settings, FakeHypervisor::default()).await,
        Err(LaunchError::Runtime(_))
    ));
    Ok(())
}
