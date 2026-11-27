#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use trogon_sandbox_contract::{DirectoryMarker, LiteralPath};
use trogon_sandbox_contract::{FsOperation, FsViolation, LeaseId};
use trogon_sandbox_contract::{FsPathEntry, FsPolicy};
use trogon_sandbox_egress_proxy::{ViolationPublishError, ViolationRateLimit, ViolationSink};
use trogon_sandbox_subprocess_worker::fs_violation::{FsDenialSink, FsViolationReporter};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

const WORKER: &str = env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker");

#[derive(Default)]
struct Collecting(Mutex<Vec<(String, FsViolation)>>);

#[async_trait]
impl ViolationSink for Collecting {
    async fn publish(&self, subject: String, payload: Vec<u8>) -> Result<(), ViolationPublishError> {
        let violation = serde_json::from_slice(&payload).map_err(|error| ViolationPublishError(error.to_string()))?;
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((subject, violation));
        Ok(())
    }
}

impl Collecting {
    async fn wait_for(&self, count: usize) -> Vec<(String, FsViolation)> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            {
                let seen = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                if seen.len() >= count || Instant::now() > deadline {
                    return seen.clone();
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn settle(&self) -> Vec<(String, FsViolation)> {
        tokio::time::sleep(Duration::from_millis(300)).await;
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("sh").is_ok()
}

fn backend() -> Result<BubblewrapBackend, Box<dyn std::error::Error>> {
    Ok(BubblewrapBackend::new(which::which("bwrap")?)
        .with_confiner(PathBuf::from(WORKER))
        .with_observer(PathBuf::from(WORKER)))
}

fn spec(
    workspace: &std::path::Path,
    script: &str,
    readonly: bool,
    policy: Option<FsPolicy>,
) -> Result<LaunchSpec, Box<dyn std::error::Error>> {
    Ok(LaunchSpec {
        binary: which::which("sh")?,
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![],
        workspace_host_path: workspace.to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: readonly,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: policy,
        platform: Default::default(),
        fs_denials: None,
    })
}

fn policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(FsPolicy::new(
        vec![],
        Vec::new(),
        vec![],
        Vec::new(),
        vec![literal("/workspace/out")?],
        vec![literal("/workspace/out/locked")?],
    )?)
}

async fn run(mut spec: LaunchSpec, sink: Arc<Collecting>, lease: &str) -> Result<i32, Box<dyn std::error::Error>> {
    let reporter = Arc::new(FsViolationReporter::new(sink, ViolationRateLimit::default()));
    let (denials, receiver) = FsDenialSink::channel();
    reporter.drain(LeaseId::new(lease), receiver);
    spec.fs_denials = Some(denials);

    let launched = backend()?.launch(&spec).await?;
    drop(spec);
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    drop(launched);
    Ok(output.status.code().unwrap_or(-1))
}

#[tokio::test]
async fn a_denied_write_under_an_fs_policy_is_reported_with_lease_path_and_operation()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    std::fs::create_dir_all(ws.path().join("out/locked"))?;
    let sink = Arc::new(Collecting::default());

    let script = "echo ok > /workspace/out/fine.txt; \
                  echo no > /workspace/beside.txt 2>/dev/null; \
                  echo no > /workspace/out/locked/inside.txt 2>/dev/null; \
                  exit 0";
    let code = run(
        spec(ws.path(), script, false, Some(policy()?))?,
        sink.clone(),
        "lease-7",
    )
    .await?;
    assert_eq!(code, 0);
    assert!(ws.path().join("out/fine.txt").exists());
    assert!(!ws.path().join("beside.txt").exists());
    assert!(!ws.path().join("out/locked/inside.txt").exists());

    let seen = sink.wait_for(2).await;
    let reported: Vec<(&str, FsOperation)> = seen.iter().map(|(_, v)| (v.path.as_str(), v.operation)).collect();
    assert!(
        reported.contains(&("/workspace/beside.txt", FsOperation::Create)),
        "{reported:?}"
    );
    assert!(
        reported.contains(&("/workspace/out/locked/inside.txt", FsOperation::Create)),
        "{reported:?}"
    );
    for (subject, violation) in &seen {
        assert_eq!(subject, "sandbox.fs.violation.lease-7");
        assert_eq!(violation.lease_id, LeaseId::new("lease-7"));
    }
    let after = sink.settle().await;
    assert!(
        after.iter().all(|(_, v)| v.path.as_str() != "/workspace/out/fine.txt"),
        "an allowed write must not be reported: {after:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_denied_write_in_a_read_only_workspace_is_reported() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    std::fs::write(ws.path().join("existing.txt"), "data")?;
    let sink = Arc::new(Collecting::default());

    let script = "echo no > /workspace/existing.txt 2>/dev/null; \
                  rm /workspace/existing.txt 2>/dev/null; \
                  echo ok > /tmp/scratch; exit 3";
    let code = run(spec(ws.path(), script, true, None)?, sink.clone(), "lease-ro").await?;
    assert_eq!(code, 3, "the payload's exit code passes through the observer");
    assert_eq!(std::fs::read_to_string(ws.path().join("existing.txt"))?, "data");

    let seen = sink.wait_for(2).await;
    let reported: Vec<(&str, FsOperation)> = seen.iter().map(|(_, v)| (v.path.as_str(), v.operation)).collect();
    assert!(
        reported.contains(&("/workspace/existing.txt", FsOperation::Create)),
        "{reported:?}"
    );
    assert!(
        reported.contains(&("/workspace/existing.txt", FsOperation::Delete)),
        "{reported:?}"
    );
    assert!(
        seen.iter().all(|(_, v)| v.path.as_str() != "/tmp/scratch"),
        "{reported:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_lease_without_a_denial_sink_runs_unobserved() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let spec = spec(ws.path(), "echo x > /workspace/a.txt; exit 5", false, None)?;
    let launched = backend()?.launch(&spec).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    assert_eq!(output.status.code(), Some(5));
    assert!(ws.path().join("a.txt").exists());
    Ok(())
}
