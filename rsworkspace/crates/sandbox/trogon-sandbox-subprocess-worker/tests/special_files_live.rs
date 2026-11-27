#![cfg(target_os = "linux")]

//! Live tests of how the sandbox ends, what it cannot escalate to, and which
//! special files it can make.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use trogon_sandbox_contract::{DirectoryMarker, LiteralPath};
use trogon_sandbox_contract::{FsPathEntry, FsPolicy};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend, LaunchedProcess};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

fn spec(ws: &Path, script: &str, fs_policy: Option<FsPolicy>) -> Result<LaunchSpec, Box<dyn std::error::Error>> {
    Ok(LaunchSpec {
        binary: which::which("python3")?,
        args: vec!["-c".to_string(), script.to_string()],
        env: vec![],
        workspace_host_path: ws.to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy,
        platform: Default::default(),
        fs_denials: None,
    })
}

fn backend() -> Result<BubblewrapBackend, Box<dyn std::error::Error>> {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"));
    Ok(BubblewrapBackend::new(which::which("bwrap")?)
        .with_confiner(helper.clone())
        .with_guest_init(helper))
}

async fn probe(
    ws: &Path,
    script: &str,
    fs_policy: Option<FsPolicy>,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let report_path = if fs_policy.is_some() {
        "out/probe.txt"
    } else {
        "probe.txt"
    };
    let prelude = format!("import sys\nsys.stdout = open('/workspace/{report_path}', 'w')\n");
    let launched = backend()?
        .launch(&spec(ws, &format!("{prelude}{script}"), fs_policy)?)
        .await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    let report = std::fs::read_to_string(ws.join(report_path)).unwrap_or_default();
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

const TICKER: &str = r#"
import time
while True:
    with open('/workspace/ticks', 'a') as f:
        f.write('x')
    time.sleep(0.1)
"#;

async fn wait_for_ticks(ws: &Path) {
    let ticks = ws.join("ticks");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ticks.exists() {
        assert!(std::time::Instant::now() < deadline, "the payload never ran");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
}

async fn assert_ticks_stopped(ws: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // Past one tick interval so a straggler would have written again.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = std::fs::metadata(ws.join("ticks"))?.len();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let after = std::fs::metadata(ws.join("ticks"))?.len();
    assert_eq!(before, after, "the payload kept running");
    Ok(())
}

fn terminate(launched: &LaunchedProcess) {
    // SAFETY: signalling a process this test started.
    unsafe { nix::libc::kill(launched.pid as i32, nix::libc::SIGTERM) };
}

#[tokio::test]
async fn sigterm_to_the_sandbox_ends_the_command_with_it() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let launched = backend()?.launch(&spec(ws.path(), TICKER, None)?).await?;
    wait_for_ticks(ws.path()).await;
    terminate(&launched);

    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await??
        .status;
    assert!(!status.success(), "{status}");
    assert_ticks_stopped(ws.path()).await?;
    Ok(())
}

#[tokio::test]
async fn the_sandbox_dies_with_the_process_that_started_it() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let path = ws.path().to_path_buf();
    // The parent-death signal is tied to the thread that spawned bubblewrap.
    let started = std::thread::spawn(move || -> Result<(), String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("runtime: {error}"))?;
        let backend = backend().map_err(|error| format!("backend: {error}"))?;
        let launch_spec = spec(&path, TICKER, None).map_err(|error| format!("spec: {error}"))?;
        let launched = runtime
            .block_on(backend.launch(&launch_spec))
            .map_err(|error| format!("launch under bwrap: {error}"))?;
        runtime.block_on(wait_for_ticks(&path));
        std::mem::forget(launched);
        Ok(())
    })
    .join()
    .map_err(|_| "starter thread panicked")?;
    started?;

    assert_ticks_stopped(ws.path()).await?;
    Ok(())
}

const ESCALATION_PROBE: &str = r#"
import os, shutil, subprocess

def status_field(name, text):
    for line in text.splitlines():
        if line.startswith(name + ":"):
            return line.split(":", 1)[1].strip()

def identity(binary, *args):
    out = subprocess.run([binary, *args, "-c",
        "import os; print(os.getuid(), [l.split(':')[1].strip() for l in open('/proc/self/status') if l.startswith('CapEff')][0])"],
        capture_output=True, text=True)
    return out.stdout.strip()

python = shutil.which("python3")
copy = "/workspace/suid-python"
shutil.copy(python, copy)
os.chmod(copy, 0o4755)
print("plain", identity(python))
print("setuid", identity(copy))

with open("/proc/self/status") as f:
    print("nonewprivs", status_field("NoNewPrivs", f.read()))

for name, argv in (("sudo", ["sudo", "-n", "sh", "-c", "grep CapEff /proc/self/status"]),
                   ("su", ["su", "-c", "grep CapEff /proc/self/status"])):
    try:
        out = subprocess.run(argv, capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=20)
        words = out.stdout.split()
        print(name, "exit" + str(out.returncode), words[-1] if words else "silent")
    except FileNotFoundError:
        print(name, "missing")
"#;

#[tokio::test]
async fn setuid_sudo_and_su_do_not_raise_privileges() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let r = probe(ws.path(), ESCALATION_PROBE, None).await?;

    assert_eq!(r["setuid"], r["plain"], "{r:?}");
    let kept = if nix::unistd::geteuid().is_root() {
        " 000001ffffffffff"
    } else {
        " 0000000000000000"
    };
    assert!(r["plain"].ends_with(kept), "{r:?}");
    assert_eq!(r["nonewprivs"], "1", "{r:?}");
    for name in ["sudo", "su"] {
        let said = &r[name];
        assert!(
            said == "missing" || !said.starts_with("exit0") || said.ends_with(kept.trim_start()),
            "{name} escalated: {r:?}"
        );
    }
    Ok(())
}

const SPECIAL_FILES_PROBE: &str = r#"
import errno, os, stat

def attempt(fn):
    try:
        return fn() or "ok"
    except OSError as e:
        return errno.errorcode[e.errno]

print("mkfifo_workspace", attempt(lambda: os.mkfifo("/workspace/pipe")))
print("is_fifo", stat.S_ISFIFO(os.stat("/workspace/pipe").st_mode))
print("mkfifo_readonly", attempt(lambda: os.mkfifo("/usr/pipe")))
print("mknod_workspace", attempt(lambda: os.mknod("/workspace/null", stat.S_IFCHR | 0o666, os.makedev(1, 3))))
print("mknod_exists", os.path.exists("/workspace/null"))
print("link_outside", attempt(lambda: os.link("/usr/bin/env", "/workspace/env-link")))
print("link_outside_exists", os.path.exists("/workspace/env-link"))
with open("/workspace/plain", "w") as f:
    f.write("body")
print("link_inside", attempt(lambda: os.link("/workspace/plain", "/workspace/plain-link")))
print("link_inside_body", open("/workspace/plain-link").read())
"#;

#[tokio::test]
async fn fifos_are_made_where_writes_are_allowed_and_devices_and_outside_links_are_not()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let r = probe(ws.path(), SPECIAL_FILES_PROBE, None).await?;

    assert_eq!(r["mkfifo_workspace"], "ok", "{r:?}");
    assert_eq!(r["is_fifo"], "True", "{r:?}");
    assert!(
        matches!(r["mkfifo_readonly"].as_str(), "EROFS" | "EACCES" | "EPERM"),
        "{r:?}"
    );
    assert_eq!(r["mknod_workspace"], "EPERM", "{r:?}");
    assert_eq!(r["mknod_exists"], "False", "{r:?}");
    assert!(
        matches!(r["link_outside"].as_str(), "EXDEV" | "EROFS" | "EPERM" | "EACCES"),
        "{r:?}"
    );
    assert_eq!(r["link_outside_exists"], "False", "{r:?}");
    assert_eq!(r["link_inside"], "ok", "{r:?}");
    assert_eq!(r["link_inside_body"], "body", "{r:?}");
    Ok(())
}

const DENIED_LINK_PROBE: &str = r#"
import errno, os

def attempt(fn):
    try:
        return fn() or "ok"
    except OSError as e:
        return errno.errorcode[e.errno]

print("link_secret", attempt(lambda: os.link("/workspace/secret.txt", "/workspace/out/secret-link")))
def leaks():
    try:
        return open("/workspace/out/secret-link").read() == "TOPSECRET"
    except OSError:
        return False
print("link_secret_leaks", leaks())
print("link_into_locked", attempt(lambda: os.link("/workspace/out/free.txt", "/workspace/out/locked/free-link")))
print("link_locked_out", attempt(lambda: os.link("/workspace/out/locked/keep.txt", "/workspace/out/keep-link")))
print("link_free", attempt(lambda: os.link("/workspace/out/free.txt", "/workspace/out/free-link")))
"#;

#[tokio::test]
async fn a_hard_link_cannot_reach_a_denied_file_or_enter_a_write_denied_directory()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    for (rel, body) in [
        ("secret.txt", "TOPSECRET"),
        ("out/free.txt", "FREE"),
        ("out/locked/keep.txt", "KEEP"),
    ] {
        let path = ws.path().join(rel);
        std::fs::create_dir_all(path.parent().ok_or("a parent directory")?)?;
        std::fs::write(path, body)?;
    }
    let policy = FsPolicy::new(
        vec![literal("/workspace/secret.txt")?],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace/out")?],
        vec![literal("/workspace/out/locked")?],
    )?;
    let r = probe(ws.path(), DENIED_LINK_PROBE, Some(policy)).await?;

    assert_ne!(r["link_secret"], "ok", "{r:?}");
    assert_eq!(r["link_secret_leaks"], "False", "{r:?}");
    assert_ne!(r["link_into_locked"], "ok", "{r:?}");
    assert!(!ws.path().join("out/locked/free-link").exists());
    assert_ne!(r["link_locked_out"], "ok", "{r:?}");
    assert_eq!(r["link_free"], "ok", "{r:?}");
    Ok(())
}
