#![cfg(target_os = "linux")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use trogon_sandbox_contract::DirectoryMarker;
use trogon_sandbox_contract::{FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_subprocess_worker::isolation::landlock::{LandlockSupport, WriteGrant, WritePlan};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec, fs_second_layer};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

const WORKER: &str = env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker");

const PROBE: &str = r#"
import errno

report = []

def attempt(name, path, content="x"):
    try:
        with open(path, "w") as f:
            f.write(content)
        report.append((name, "ok"))
    except OSError as e:
        report.append((name, errno.errorcode[e.errno]))

attempt("write_granted", "/workspace/out/new.txt")
attempt("write_beside_grant", "/workspace/other.txt")
attempt("write_tmp", "/tmp/probe")
attempt("write_dev_null", "/dev/null")
with open("/proc/self/oom_score_adj") as current:
    attempt("write_proc_self", "/proc/self/oom_score_adj", current.read())

with open("/workspace/out/probe.txt", "w") as out:
    for name, value in report:
        out.write(name + " " + value + "\n")
"#;

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn landlock_enforced() -> bool {
    fs_second_layer().is_enforced()
}

fn guest(raw: &str) -> Result<LiteralPath, Box<dyn std::error::Error>> {
    Ok(LiteralPath::new(raw)?)
}

fn policy() -> Result<FsPolicy, Box<dyn std::error::Error>> {
    Ok(FsPolicy::new(
        vec![],
        Vec::new(),
        vec![],
        Vec::new(),
        vec![literal("/workspace")?, literal("/tmp")?],
        vec![],
    )?)
}

fn workspace() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
    let ws = tempfile::tempdir()?;
    std::fs::create_dir(ws.path().join("out"))?;
    Ok(ws)
}

fn backend() -> Result<BubblewrapBackend, Box<dyn std::error::Error>> {
    Ok(BubblewrapBackend::new(which::which("bwrap")?).with_confiner(PathBuf::from(WORKER)))
}

async fn probe(ws: &Path, backend: BubblewrapBackend) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let spec = LaunchSpec {
        binary: which::which("python3")?,
        args: vec!["-c".to_string(), PROBE.to_string()],
        env: vec![],
        workspace_host_path: ws.to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: Some(policy()?),
        platform: Default::default(),
        fs_denials: None,
    };
    let launched = backend.launch(&spec).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    let report = std::fs::read_to_string(ws.join("out/probe.txt")).unwrap_or_default();
    assert!(
        output.status.success(),
        "probe exited with {}: {report}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(report
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

#[tokio::test]
async fn the_bind_alone_permits_the_write_that_landlock_refuses() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = workspace()?;
    let r = probe(
        ws.path(),
        backend()?.with_landlock_support(LandlockSupport::Unavailable),
    )
    .await?;
    assert_eq!(r["write_beside_grant"], "ok", "{r:?}");
    assert!(ws.path().join("other.txt").exists());
    Ok(())
}

#[tokio::test]
async fn landlock_keeps_every_write_the_mounts_allow() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() || !landlock_enforced() {
        return Ok(());
    }
    let ws = workspace()?;
    let r = probe(ws.path(), backend()?).await?;
    assert_eq!(r["write_granted"], "ok", "{r:?}");
    assert_eq!(r["write_beside_grant"], "ok", "{r:?}");
    assert_eq!(r["write_tmp"], "ok", "{r:?}");
    assert_eq!(r["write_dev_null"], "ok", "{r:?}");
    assert_eq!(r["write_proc_self"], "ok", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn a_write_outside_the_allow_list_fails_even_when_a_bind_permits_it() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() || !landlock_enforced() {
        return Ok(());
    }
    let ws = workspace()?;
    let mis_planned = WritePlan::new([
        WriteGrant::tree(guest("/workspace/out")?),
        WriteGrant::tree(guest("/tmp")?),
        WriteGrant::existing_files(guest("/dev")?),
    ]);
    let r = probe(ws.path(), backend()?.with_write_plan(mis_planned)).await?;
    assert_eq!(r["write_granted"], "ok", "{r:?}");
    assert_eq!(
        r["write_beside_grant"], "EACCES",
        "the workspace bind is read-write, so only Landlock can refuse: {r:?}"
    );
    assert!(!ws.path().join("other.txt").exists());
    assert_eq!(r["write_tmp"], "ok", "{r:?}");
    assert_eq!(r["write_dev_null"], "ok", "{r:?}");
    Ok(())
}

#[test]
fn the_launcher_refuses_writes_outside_its_roots_without_bubblewrap() -> Result<(), Box<dyn std::error::Error>> {
    let Some(abi) = fs_second_layer().abi() else {
        return Ok(());
    };
    let dir = tempfile::tempdir()?;
    let allowed = dir.path().join("allowed");
    let outside = dir.path().join("outside");
    std::fs::create_dir(&allowed)?;
    std::fs::create_dir(&outside)?;

    let run = |target: &Path| -> Result<std::process::Output, std::io::Error> {
        Command::new(WORKER)
            .args(["--confine", "--abi", &abi.version().to_string(), "--write"])
            .arg(&allowed)
            .args(["--", "/bin/sh", "-c", "echo x > \"$0\""])
            .arg(target)
            .output()
    };

    assert!(run(&allowed.join("f"))?.status.success());
    assert!(allowed.join("f").exists());
    assert!(!run(&outside.join("f"))?.status.success());
    assert!(!outside.join("f").exists());
    Ok(())
}

#[test]
fn the_launcher_fails_closed_when_the_ruleset_cannot_be_built() -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new(WORKER)
        .args(["--confine", "--abi", "0", "--", "/bin/true"])
        .output()?;
    assert!(!output.status.success());
    Ok(())
}
