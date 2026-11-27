use trogon_sandbox_contract::LeaseId;

use super::*;
use crate::config::StateDir;

fn dir(tmp: &Path) -> Result<VmDir, Box<dyn std::error::Error>> {
    Ok(VmDir::for_lease(&StateDir::new(tmp), &LeaseId::new("l1"))?)
}

#[tokio::test]
async fn a_missing_binary_is_a_launch_error() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = dir(tmp.path())?;
    dir.create().await?;
    let launcher = DirectLauncher::new(FirecrackerBinary::new("/nonexistent/firecracker"));
    let error = launcher.launch(&dir).await.err().ok_or("expected an error")?;
    assert!(matches!(error, ProviderError::Launch(_)), "{error}");
    Ok(())
}

#[tokio::test]
async fn a_vmm_that_exits_early_is_reported_before_the_timeout() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = dir(tmp.path())?;
    dir.create().await?;
    let launcher = DirectLauncher::new(FirecrackerBinary::new("false"));
    let mut process = launcher.launch(&dir).await?;
    let error = process
        .wait_for_api_socket(&dir.api_socket(), Duration::from_secs(10))
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, ProviderError::VmmExited(_)), "{error}");
    Ok(())
}

#[tokio::test]
async fn a_socket_that_never_appears_times_out() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = dir(tmp.path())?;
    dir.create().await?;
    let launcher = DirectLauncher::new(FirecrackerBinary::new("sleep"));
    // `sleep` treats `--api-sock` as an invalid option and exits, which would hit the
    // early-exit path, so hold the process with a binary that ignores its arguments.
    let mut process = VmProcess::from_child(Command::new("sleep").arg("30").kill_on_drop(true).spawn()?);
    let _ = launcher;
    let error = process
        .wait_for_api_socket(&dir.api_socket(), Duration::from_millis(100))
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, ProviderError::ApiSocketTimeout { .. }), "{error}");
    process.stop().await;
    Ok(())
}

fn holding_process() -> Result<VmProcess, Box<dyn std::error::Error>> {
    Ok(VmProcess::from_child(
        Command::new("sleep").arg("30").kill_on_drop(true).spawn()?,
    ))
}

#[tokio::test]
async fn a_socket_path_that_refuses_connections_is_not_ready() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = dir(tmp.path())?;
    dir.create().await?;
    std::fs::write(dir.api_socket(), b"")?;
    let mut process = holding_process()?;
    let error = process
        .wait_for_api_socket(&dir.api_socket(), Duration::from_millis(200))
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, ProviderError::ApiSocketTimeout { .. }), "{error}");
    process.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_listening_socket_is_ready() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = dir(tmp.path())?;
    dir.create().await?;
    let _listener = tokio::net::UnixListener::bind(dir.api_socket())?;
    let mut process = holding_process()?;
    process
        .wait_for_api_socket(&dir.api_socket(), Duration::from_secs(5))
        .await?;
    process.stop().await;
    Ok(())
}

#[tokio::test]
async fn stop_is_idempotent() -> Result<(), Box<dyn std::error::Error>> {
    let mut process = VmProcess::from_child(Command::new("sleep").arg("30").kill_on_drop(true).spawn()?);
    process.stop().await;
    process.stop().await;
    Ok(())
}
