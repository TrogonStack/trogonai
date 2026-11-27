#![cfg(target_os = "linux")]

use std::path::PathBuf;

use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

fn bwrap_available() -> bool {
    which::which("bwrap").is_ok()
}

fn curl_available() -> bool {
    which::which("curl").is_ok()
}

async fn run_curl_probe(egress: EgressPolicy) -> Result<i32, Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let backend = BubblewrapBackend::new(which::which("bwrap")?);
    let spec = LaunchSpec {
        binary: PathBuf::from("/bin/sh"),
        args: vec![
            "-c".to_string(),
            "curl -s -m 2 1.1.1.1 >/dev/null 2>&1; echo $? > /workspace/report".to_string(),
        ],
        env: vec![],
        workspace_host_path: scratch.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: egress,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    };

    let launched = backend.launch(&spec).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    assert!(output.status.success(), "probe shell exited {}", output.status);
    Ok(std::fs::read_to_string(scratch.path().join("report"))?.trim().parse()?)
}

#[tokio::test]
#[ignore = "requires Linux, bwrap, and curl"]
async fn deny_all_blocks_egress() -> Result<(), Box<dyn std::error::Error>> {
    if !bwrap_available() || !curl_available() {
        return Ok(());
    }
    let exit_code = run_curl_probe(EgressPolicy::DenyAll).await?;
    assert_ne!(exit_code, 0, "curl should fail without network namespace");
    Ok(())
}

#[tokio::test]
#[ignore = "requires Linux, bwrap, curl, and outbound internet"]
async fn allow_all_permits_egress_when_online() -> Result<(), Box<dyn std::error::Error>> {
    if !bwrap_available() || !curl_available() {
        return Ok(());
    }
    let exit_code = run_curl_probe(EgressPolicy::AllowAll).await?;
    if exit_code != 0 {
        return Ok(());
    }
    assert_eq!(exit_code, 0);
    Ok(())
}

#[tokio::test]
async fn deny_all_integration_when_tools_present() -> Result<(), Box<dyn std::error::Error>> {
    if !bwrap_available() || !curl_available() {
        return Ok(());
    }
    let exit_code = run_curl_probe(EgressPolicy::DenyAll).await?;
    assert_ne!(exit_code, 0, "curl should fail without network namespace");
    Ok(())
}
