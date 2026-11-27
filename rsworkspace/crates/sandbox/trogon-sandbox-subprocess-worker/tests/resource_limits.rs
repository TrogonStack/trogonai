#![cfg(target_os = "linux")]

use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use trogon_sandbox_subprocess_worker::isolation::limits::{ProcessLimit, ResourceEnforcer, ResourceLimits};
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

const MEMORY_HOG: &str = r#"
chunks = []
for _ in range(64):
    chunks.append(bytearray(b"\x01") * (16 << 20))
open("/workspace/done", "w").write("survived")
"#;

const MODEST_ALLOCATION: &str = r#"
chunk = bytearray(b"\x01") * (4 << 20)
open("/workspace/done", "w").write("ok")
"#;

const FORK_BOMB: &str = r#"
import os, time
forks = 0
outcome = "unbounded"
try:
    while forks < 2000:
        if os.fork() == 0:
            time.sleep(30)
            os._exit(0)
        forks += 1
except OSError as e:
    outcome = "blocked"
open("/workspace/probe.txt", "w").write(f"{outcome} {forks}")
"#;

const CPU_LOOP: &str = r#"
while True:
    pass
"#;

struct Outcome {
    status: ExitStatus,
    workspace: tempfile::TempDir,
    elapsed: Duration,
}

impl Outcome {
    fn file(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.workspace.path().join(name)).ok()
    }
}

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn cgroup_enforcer() -> Option<ResourceEnforcer> {
    let enforcer = ResourceEnforcer::detect();
    enforcer.lease_root()?;
    Some(enforcer)
}

async fn run(
    script: &str,
    limits: ResourceLimits,
    enforcer: ResourceEnforcer,
) -> Result<Outcome, Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let backend = BubblewrapBackend::with_resource_enforcer(which::which("bwrap")?, enforcer);
    let spec = LaunchSpec {
        binary: which::which("python3")?,
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![],
        workspace_host_path: workspace.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits,
        fs_policy: None,
        platform: Default::default(),
        fs_denials: None,
    };

    let started = Instant::now();
    let launched = backend.launch(&spec).await?;
    let mut child = launched.child.lock().await.take().ok_or("child handle")?;
    let status = match tokio::time::timeout(Duration::from_secs(60), child.wait()).await {
        Ok(status) => status?,
        Err(_) => {
            let _ = child.kill().await;
            return Err("probe was not stopped by its resource limits".into());
        }
    };
    let elapsed = started.elapsed();
    launched.limits.release().await;
    Ok(Outcome {
        status,
        workspace,
        elapsed,
    })
}

fn memory_limits() -> ResourceLimits {
    ResourceLimits::from_spec(0, 128)
}

fn process_limits() -> Result<ResourceLimits, Box<dyn std::error::Error>> {
    Ok(ResourceLimits::default().with_processes(ProcessLimit::new(64.try_into()?)))
}

async fn assert_memory_hog_stopped(enforcer: ResourceEnforcer) -> Result<(), Box<dyn std::error::Error>> {
    let outcome = run(MEMORY_HOG, memory_limits(), enforcer).await?;
    assert!(!outcome.status.success(), "hog exited cleanly");
    assert_eq!(outcome.file("done"), None, "hog finished allocating");
    Ok(())
}

async fn assert_modest_allocation_allowed(enforcer: ResourceEnforcer) -> Result<(), Box<dyn std::error::Error>> {
    let outcome = run(MODEST_ALLOCATION, memory_limits(), enforcer).await?;
    assert!(outcome.status.success(), "status {}", outcome.status);
    assert_eq!(outcome.file("done").as_deref(), Some("ok"));
    Ok(())
}

async fn assert_fork_bomb_stopped(enforcer: ResourceEnforcer) -> Result<(), Box<dyn std::error::Error>> {
    let outcome = run(FORK_BOMB, process_limits()?, enforcer).await?;
    let report = outcome.file("probe.txt").unwrap_or_default();
    let (verdict, forks) = report.split_once(' ').ok_or("probe report")?;
    let forks: u32 = forks.parse()?;
    assert_eq!(verdict, "blocked", "report: {report}");
    assert!(forks < 64, "forked {forks} times");
    Ok(())
}

async fn assert_cpu_loop_killed(enforcer: ResourceEnforcer) -> Result<(), Box<dyn std::error::Error>> {
    let limits = ResourceLimits::from_spec(1, 0);
    let outcome = run(CPU_LOOP, limits, enforcer).await?;
    assert!(
        outcome.status.signal().is_some() || outcome.status.code().is_some_and(|c| c > 128),
        "status {}",
        outcome.status
    );
    assert!(outcome.elapsed < Duration::from_secs(30), "took {:?}", outcome.elapsed);
    Ok(())
}

#[tokio::test]
async fn rlimit_stops_memory_hog() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_memory_hog_stopped(ResourceEnforcer::rlimit_only()).await?;
    Ok(())
}

#[tokio::test]
async fn rlimit_allows_modest_allocation() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_modest_allocation_allowed(ResourceEnforcer::rlimit_only()).await?;
    Ok(())
}

#[tokio::test]
async fn rlimit_stops_fork_bomb() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    if unsafe { nix::libc::geteuid() } == 0 {
        return Ok(());
    }
    assert_fork_bomb_stopped(ResourceEnforcer::rlimit_only()).await?;
    Ok(())
}

#[tokio::test]
async fn rlimit_kills_cpu_loop() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_cpu_loop_killed(ResourceEnforcer::rlimit_only()).await?;
    Ok(())
}

#[tokio::test]
async fn cgroup_stops_memory_hog() -> Result<(), Box<dyn std::error::Error>> {
    let (true, Some(enforcer)) = (tools_available(), cgroup_enforcer()) else {
        return Ok(());
    };
    assert_memory_hog_stopped(enforcer).await?;
    Ok(())
}

#[tokio::test]
async fn cgroup_allows_modest_allocation() -> Result<(), Box<dyn std::error::Error>> {
    let (true, Some(enforcer)) = (tools_available(), cgroup_enforcer()) else {
        return Ok(());
    };
    assert_modest_allocation_allowed(enforcer).await?;
    Ok(())
}

#[tokio::test]
async fn cgroup_stops_fork_bomb() -> Result<(), Box<dyn std::error::Error>> {
    let (true, Some(enforcer)) = (tools_available(), cgroup_enforcer()) else {
        return Ok(());
    };
    assert_fork_bomb_stopped(enforcer).await?;
    Ok(())
}

#[tokio::test]
async fn cgroup_kills_cpu_loop() -> Result<(), Box<dyn std::error::Error>> {
    let (true, Some(enforcer)) = (tools_available(), cgroup_enforcer()) else {
        return Ok(());
    };
    assert_cpu_loop_killed(enforcer).await?;
    Ok(())
}

#[tokio::test]
async fn cgroup_is_removed_when_the_lease_ends() -> Result<(), Box<dyn std::error::Error>> {
    let (true, Some(enforcer)) = (tools_available(), cgroup_enforcer()) else {
        return Ok(());
    };
    let root = enforcer.lease_root().ok_or("lease root")?.to_path_buf();
    run(FORK_BOMB, process_limits()?, enforcer).await?;
    let leftovers: Vec<_> = std::fs::read_dir(&root)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();
    assert!(leftovers.is_empty(), "leaked cgroups: {leftovers:?}");
    Ok(())
}
