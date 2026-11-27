use trogon_sandbox_contract::LeaseId;

use super::*;
use crate::config::StateDir;

fn dir(tmp: &Path) -> Result<VmDir, Box<dyn std::error::Error>> {
    Ok(VmDir::for_lease(&StateDir::new(tmp), &LeaseId::new("l1"))?)
}

fn holding_process() -> Result<VmProcess, Box<dyn std::error::Error>> {
    Ok(VmProcess::supervise(
        Command::new("sleep").arg("30").kill_on_drop(true).spawn()?,
        Signal::Child,
    ))
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
    // `sleep` treats `--api-sock` as an invalid option and exits, which would hit the
    // early-exit path, so hold the process with a binary that ignores its arguments.
    let mut process = holding_process()?;
    let error = process
        .wait_for_api_socket(&dir.api_socket(), Duration::from_millis(100))
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, ProviderError::ApiSocketTimeout { .. }), "{error}");
    process.stop().await;
    Ok(())
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
    let mut process = holding_process()?;
    process.stop().await;
    process.stop().await;
    assert!(process.exit().status().is_some());
    Ok(())
}

#[tokio::test]
async fn a_vmm_that_dies_on_its_own_is_reported_without_polling() -> Result<(), Box<dyn std::error::Error>> {
    let mut process = holding_process()?;
    let pid = process.pid().ok_or("pid missing")?;
    let exit = process.exit();
    assert!(exit.status().is_none());
    Command::new("kill").arg("-KILL").arg(pid.to_string()).status().await?;
    let status = tokio::time::timeout(Duration::from_secs(5), exit.wait()).await?;
    assert!(status.contains("signal"), "{status}");
    process.stop().await;
    Ok(())
}

#[tokio::test]
async fn dropping_the_handle_kills_the_vmm() -> Result<(), Box<dyn std::error::Error>> {
    let process = holding_process()?;
    let exit = process.exit();
    drop(process);
    tokio::time::timeout(Duration::from_secs(5), exit.wait()).await?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_recorded_pid_is_killed_only_when_it_names_the_lease_socket() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempfile::tempdir()?;
    let dir = dir(tmp.path())?;
    dir.create().await?;
    let socket = dir.api_socket();
    let mut incidental = Command::new("sh")
        .args(["-c", "while :; do sleep 1; done"])
        .arg(&socket)
        .kill_on_drop(true)
        .spawn()?;
    let mut unrelated = holding_process()?;
    tokio::fs::write(dir.pid_file(), unrelated.pid().ok_or("pid missing")?.to_string()).await?;
    reap_orphan(&dir).await?;
    assert!(unrelated.exit().status().is_none());
    assert!(
        incidental.try_wait()?.is_none(),
        "an incidental socket argument is not ownership proof"
    );

    let mut named = Command::new("sh")
        .args(["-c", "while :; do sleep 1; done", "--api-sock"])
        .arg(&socket)
        .kill_on_drop(true)
        .spawn()?;
    tokio::fs::write(dir.pid_file(), named.id().ok_or("the child has no pid")?.to_string()).await?;
    reap_orphan(&dir).await?;
    let status = tokio::time::timeout(Duration::from_secs(5), named.wait()).await??;
    assert!(!status.success());
    incidental.kill().await?;
    unrelated.stop().await;
    Ok(())
}
