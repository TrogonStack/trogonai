#![cfg(target_os = "linux")]

use std::error::Error;
use std::path::PathBuf;

use trogon_sandbox_subprocess_worker::isolation::resource_limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

#[tokio::test]
async fn workspace_bind_allows_writes() -> Result<(), Box<dyn Error>> {
    let Ok(bwrap) = which::which("bwrap") else {
        return Ok(());
    };

    let scratch = tempfile::tempdir()?;
    let backend = BubblewrapBackend::new(bwrap);
    let spec = LaunchSpec {
        binary: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string(), "echo hello > /workspace/out.txt".to_string()],
        env: vec![],
        workspace_host_path: scratch.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    };

    let launched = backend.launch(&spec).await?;
    let mut guard = launched.child.lock().await;
    let child = guard.as_mut().ok_or("the launch holds a child handle")?;
    let status = child.wait().await?;
    assert!(status.success(), "shell command should succeed");

    let contents = std::fs::read_to_string(scratch.path().join("out.txt"))?;
    assert_eq!(contents.trim(), "hello");
    Ok(())
}
