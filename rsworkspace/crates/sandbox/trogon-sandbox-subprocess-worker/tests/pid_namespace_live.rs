#![cfg(target_os = "linux")]

//! Live tests of PID namespace isolation, names with a backslash, names that are
//! no text and stale mount points. Each runs a payload under
//! bubblewrap and reads back what it could do.

use std::collections::BTreeMap;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use trogon_sandbox_contract::{DirectoryMarker, GlobPattern};
use trogon_sandbox_contract::{FilesystemOptions, FsPathEntry, FsPolicy, LiteralPath};
use trogon_sandbox_subprocess_worker::isolation::host_tree::LinuxHostTree;
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::mount_plan::{GuestLayout, MountAccess, plan_mounts_with};
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

fn literal(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Literal {
        path: LiteralPath::new(raw)?,
        directory: DirectoryMarker::default(),
    })
}

fn glob(raw: &str) -> Result<FsPathEntry, Box<dyn std::error::Error>> {
    Ok(FsPathEntry::Glob {
        pattern: GlobPattern::new(raw)?,
    })
}

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

async fn launch(
    ws: &Path,
    binary: PathBuf,
    args: Vec<String>,
    fs_policy: Option<FsPolicy>,
) -> Result<(ExitStatus, BTreeMap<String, String>), Box<dyn std::error::Error>> {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"));
    let backend = BubblewrapBackend::new(which::which("bwrap")?)
        .with_confiner(helper.clone())
        .with_guest_init(helper);
    let spec = LaunchSpec {
        binary,
        args,
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
    };
    let launched = backend.launch(&spec).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let output = child.wait_with_output().await?;
    let report = std::fs::read_to_string(ws.join("probe.txt")).unwrap_or_default();
    let results = report
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Ok((output.status, results))
}

async fn probe(
    ws: &Path,
    script: &str,
    fs_policy: Option<FsPolicy>,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let prelude = "import sys\nsys.stdout = open('/workspace/probe.txt', 'w')\n";
    let (status, results) = launch(
        ws,
        which::which("python3")?,
        vec!["-c".to_string(), format!("{prelude}{script}")],
        fs_policy,
    )
    .await?;
    assert!(status.success(), "probe exited with {status}: {results:?}");
    Ok(results)
}

async fn exit_code_of(shell: &str) -> Result<Option<i32>, Box<dyn std::error::Error>> {
    let ws = tempfile::tempdir()?;
    let (status, _) = launch(
        ws.path(),
        which::which("sh")?,
        vec!["-c".to_string(), shell.to_string()],
        None,
    )
    .await?;
    Ok(status.code())
}

const PID_PROBE: &str = r#"
import ctypes, os

libc = ctypes.CDLL(None, use_errno=True)

pids = sorted(int(n) for n in os.listdir("/proc") if n.isdigit())
print("pids", ",".join(map(str, pids)))
print("self", os.getpid())

r = libc.ptrace(16, 1, 0, 0)
print("ptrace_pid1", r, ctypes.get_errno())

try:
    mem = open("/proc/1/mem", "r+b")
    print("proc1_mem", "OPENED")
    try:
        start = int(open("/proc/1/maps").readline().split("-")[0], 16)
        mem.seek(start)
        mem.read(1)
        print("proc1_mem_read", "ok")
    except Exception as e:
        print("proc1_mem_read", type(e).__name__)
    print("proc1_comm", open("/proc/1/comm").read().strip())
except PermissionError:
    print("proc1_mem", "DENIED")
except OSError as e:
    print("proc1_mem", "ERR" + str(e.errno))

class iovec(ctypes.Structure):
    _fields_ = [("base", ctypes.c_void_p), ("len", ctypes.c_size_t)]

buf = ctypes.create_string_buffer(b"x")
local = iovec(ctypes.cast(buf, ctypes.c_void_p).value, 1)
remote = iovec(0x1000, 1)
r = libc.process_vm_writev(1, ctypes.byref(local), 1, ctypes.byref(remote), 1, 0)
print("vm_writev_pid1", r, ctypes.get_errno())

p = (ctypes.c_byte * 256)()
r = libc.syscall(425, 4, p)
print("io_uring_setup", r, ctypes.get_errno())
r = libc.syscall(426, 99, 0, 0, 0, 0, 0)
print("io_uring_enter", r, ctypes.get_errno())
"#;

#[tokio::test]
async fn the_payload_sees_only_its_own_pid_namespace() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let r = probe(ws.path(), PID_PROBE, None).await?;

    let pids: Vec<u32> = r["pids"].split(',').map(str::parse).collect::<Result<_, _>>()?;
    assert_eq!(pids[0], 1, "{pids:?}");
    assert!(
        *pids.iter().max().ok_or("no pids")? < 20,
        "a host pid leaked into /proc: {pids:?}"
    );
    let own: u32 = r["self"].parse()?;
    assert!(own < 20, "the payload runs under a host pid: {own}");
    Ok(())
}

#[tokio::test]
async fn pid_one_cannot_be_ptraced_or_written_from_the_payload() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let r = probe(ws.path(), PID_PROBE, None).await?;

    let failed_with = |key: &str, errnos: &[&str]| -> Result<(), Box<dyn std::error::Error>> {
        let mut parts = r[key].split(' ');
        let rc = parts.next().ok_or("return code")?;
        let errno = parts.next().ok_or("errno")?;
        assert_eq!(rc, "-1", "{key}: {r:?}");
        assert!(errnos.contains(&errno), "{key}: errno {errno} in {r:?}");
        Ok(())
    };
    failed_with("ptrace_pid1", &["1", "13"])?;
    failed_with("vm_writev_pid1", &["1", "3", "13"])?;
    Ok(())
}

#[tokio::test]
async fn proc_one_mem_cannot_be_opened_for_writing_from_the_payload() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let r = probe(ws.path(), PID_PROBE, None).await?;

    assert_eq!(r["proc1_mem"], "DENIED", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn io_uring_cannot_be_set_up_or_entered() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let r = probe(ws.path(), PID_PROBE, None).await?;

    assert_eq!(r["io_uring_setup"], "-1 1", "{r:?}");
    assert_eq!(r["io_uring_enter"], "-1 1", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn the_exit_code_of_the_command_is_the_exit_code_of_the_sandbox() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    for code in [0, 1, 42, 127] {
        assert_eq!(exit_code_of(&format!("exit {code}")).await?, Some(code));
    }
    Ok(())
}

#[tokio::test]
async fn a_signal_exit_is_reported_as_128_plus_the_signal() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_eq!(exit_code_of("kill -TERM $$").await?, Some(128 + 15));
    Ok(())
}

#[tokio::test]
async fn the_sandbox_ends_with_the_command_though_a_background_process_runs() -> Result<(), Box<dyn std::error::Error>>
{
    if !tools_available() {
        return Ok(());
    }
    let started = std::time::Instant::now();
    assert_eq!(exit_code_of("sleep 100 & exit 5").await?, Some(5));
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    Ok(())
}

#[tokio::test]
async fn an_orphaned_grandchild_does_not_change_the_exit_code() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    assert_eq!(exit_code_of("(sleep 0.2 &); exit 7").await?, Some(7));
    Ok(())
}

const BACKSLASH_PROBE: &str = r#"
import errno, os

def read(path):
    try:
        with open(path) as f:
            return "ok:" + f.read().strip()
    except OSError as e:
        return errno.errorcode[e.errno]

def append(path):
    try:
        with open(path, "a") as f:
            f.write("appended")
        return "ok"
    except OSError as e:
        return errno.errorcode[e.errno]

print("read_backslash", read("/workspace/tree/a\\b"))
print("read_slash", read("/workspace/tree/a/b"))
print("append_backslash_dir", append("/workspace/tree/c\\d/file"))
print("append_slash_dir", append("/workspace/tree/c/d/file"))
print("append_plain", append("/workspace/tree/plain"))
print("read_plain", read("/workspace/tree/plain"))
print("read_through_link", read("/workspace/link/a\\b"))
"#;

fn backslash_tree(ws: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let write = |rel: &str, body: &str| -> Result<(), Box<dyn std::error::Error>> {
        let path = ws.join(rel);
        std::fs::create_dir_all(path.parent().ok_or("a parent directory")?)?;
        std::fs::write(path, body)?;
        Ok(())
    };
    write("tree/a\\b", "BACKSLASH")?;
    write("tree/a/b", "SLASH")?;
    write("tree/c\\d/file", "BACKSLASH-DIR")?;
    write("tree/c/d/file", "SLASH-DIR")?;
    write("tree/plain", "PLAIN")?;
    std::os::unix::fs::symlink("tree", ws.join("link"))?;
    Ok(())
}

#[tokio::test]
async fn a_read_deny_of_a_backslash_name_leaves_the_name_that_spells_a_path() -> Result<(), Box<dyn std::error::Error>>
{
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    backslash_tree(ws.path())?;
    let policy = FsPolicy::new(
        vec![literal("/workspace/tree/a\\b")?],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        Vec::new(),
    )?;

    let r = probe(ws.path(), BACKSLASH_PROBE, Some(policy)).await?;

    assert_eq!(r["read_backslash"], "EACCES", "{r:?}");
    assert_eq!(r["read_through_link"], "EACCES", "{r:?}");
    assert_eq!(r["read_slash"], "ok:SLASH", "{r:?}");
    assert!(r["read_plain"].starts_with("ok:PLAIN"), "{r:?}");
    Ok(())
}

#[tokio::test]
async fn a_read_deny_pattern_ending_in_a_backslash_name_spares_the_path_spelling()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    backslash_tree(ws.path())?;
    let policy = FsPolicy::new(
        vec![glob("/workspace/**/c\\d/file")?],
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        Vec::new(),
    )?;

    let r = probe(
        ws.path(),
        r#"
import errno
def read(path):
    try:
        with open(path) as f:
            return "ok:" + f.read().strip()
    except OSError as e:
        return errno.errorcode[e.errno]
print("backslash_dir", read("/workspace/tree/c\\d/file"))
print("slash_dir", read("/workspace/tree/c/d/file"))
"#,
        Some(policy),
    )
    .await?;

    assert_eq!(r["backslash_dir"], "EACCES", "{r:?}");
    assert_eq!(r["slash_dir"], "ok:SLASH-DIR", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn a_write_deny_of_a_backslash_name_leaves_the_rest_writable() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    backslash_tree(ws.path())?;
    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        vec![literal("/workspace/tree/c\\d/file")?],
    )?;

    let r = probe(ws.path(), BACKSLASH_PROBE, Some(policy)).await?;

    assert_eq!(r["append_backslash_dir"], "EROFS", "{r:?}");
    assert_eq!(r["append_slash_dir"], "ok", "{r:?}");
    assert_eq!(r["append_plain"], "ok", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn a_write_deny_of_an_absent_backslash_name_holds_and_spares_the_rest() -> Result<(), Box<dyn std::error::Error>>
{
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    std::fs::create_dir_all(ws.path().join("tree"))?;
    std::fs::write(ws.path().join("tree/plain"), "PLAIN")?;
    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        vec![literal("/workspace/tree/c\\d/file")?],
    )?;

    let r = probe(ws.path(), BACKSLASH_PROBE, Some(policy)).await?;

    assert_ne!(r["append_backslash_dir"], "ok", "{r:?}");
    assert_eq!(r["append_plain"], "ok", "{r:?}");
    Ok(())
}

#[tokio::test]
async fn the_shell_start_up_file_in_a_backslash_directory_stays_read_only() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    for rel in ["c\\d/.bashrc", "c\\d/plain", "c/d/.bashrc"] {
        let path = ws.path().join(rel);
        std::fs::create_dir_all(path.parent().ok_or("a parent directory")?)?;
        std::fs::write(path, "")?;
    }
    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        Vec::new(),
    )?;

    let r = probe(
        ws.path(),
        r#"
import errno
def append(path):
    try:
        with open(path, "a") as f:
            f.write("appended")
        return "ok"
    except OSError as e:
        return errno.errorcode[e.errno]
print("bashrc_backslash", append("/workspace/c\\d/.bashrc"))
print("plain_backslash", append("/workspace/c\\d/plain"))
print("bashrc_slash", append("/workspace/c/d/.bashrc"))
"#,
        Some(policy),
    )
    .await?;

    assert_ne!(r["bashrc_backslash"], "ok", "{r:?}");
    assert_eq!(r["plain_backslash"], "ok", "{r:?}");
    assert_ne!(r["bashrc_slash"], "ok", "{r:?}");
    Ok(())
}

const STALE_PROBE: &str = r#"
import errno
try:
    with open("/workspace/repo/.git/config.lock", "a") as f:
        f.write("x")
    print("write", "ok")
except OSError as e:
    print("write", errno.errorcode[e.errno])
"#;

async fn stale_lock(mode: u32, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    let ws = tempfile::tempdir()?;
    let lock = ws.path().join("repo/.git/config.lock");
    std::fs::create_dir_all(lock.parent().ok_or("a parent directory")?)?;
    std::fs::write(&lock, content)?;
    std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(mode))?;
    let policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        vec![literal("/workspace/repo/.git/config.lock")?],
    )?;

    let r = probe(ws.path(), STALE_PROBE, Some(policy)).await?;

    assert_ne!(r["write"], "ok", "{r:?}");
    assert_eq!(std::fs::read_to_string(&lock)?, content);
    assert_eq!(std::fs::metadata(&lock)?.mode() & 0o777, mode);
    Ok(())
}

#[tokio::test]
async fn a_read_only_empty_file_at_a_write_deny_path_cannot_be_written_and_is_left_as_found()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    stale_lock(0o444, "").await?;
    Ok(())
}

#[tokio::test]
async fn an_empty_file_with_write_bits_at_a_write_deny_path_cannot_be_written_and_is_left_as_found()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    stale_lock(0o644, "").await?;
    Ok(())
}

#[tokio::test]
async fn a_read_only_file_with_content_at_a_write_deny_path_cannot_be_written_and_is_left_as_found()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    stale_lock(0o444, "x").await?;
    Ok(())
}

#[tokio::test]
async fn a_file_that_was_there_before_the_launch_is_not_reported_for_clean_up() -> Result<(), Box<dyn std::error::Error>>
{
    if !tools_available() {
        return Ok(());
    }
    let ws = tempfile::tempdir()?;
    let lock = ws.path().join("repo/.git/config.lock");
    std::fs::create_dir_all(lock.parent().ok_or("a parent directory")?)?;
    std::fs::write(&lock, "")?;
    let spec_policy = FsPolicy::new(
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        vec![literal("/workspace/repo/.git/config.lock")?],
    )?;
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"));
    let backend = BubblewrapBackend::new(which::which("bwrap")?)
        .with_confiner(helper.clone())
        .with_guest_init(helper);
    let spec = LaunchSpec {
        binary: which::which("true")?,
        args: vec![],
        env: vec![],
        workspace_host_path: ws.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: Some(spec_policy),
        platform: Default::default(),
        fs_denials: None,
    };

    let launched = backend.launch(&spec).await?;
    let placeholders = launched.placeholders.clone();
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    child.wait_with_output().await?;

    assert!(!placeholders.contains(&lock), "{placeholders:?}");
    assert!(lock.exists());
    Ok(())
}

fn non_text(before: &str, after: &str) -> std::ffi::OsString {
    let mut bytes = before.as_bytes().to_vec();
    bytes.push(0xff);
    bytes.extend_from_slice(after.as_bytes());
    std::ffi::OsString::from_vec(bytes)
}

fn plan_with_non_text_link(
    deny_read: &[&str],
) -> Result<trogon_sandbox_subprocess_worker::isolation::mount_plan::MountPlan, Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let base = root.path().canonicalize()?;
    std::fs::create_dir_all(base.join(non_text("n", "")).join("sub"))?;
    std::fs::write(base.join(non_text("n", "")).join("file"), "")?;
    std::os::unix::fs::symlink(non_text("n", ""), base.join("link"))?;
    let mut layout = GuestLayout::new();
    layout.bind(LiteralPath::new("/workspace")?, base, MountAccess::ReadWrite);
    let policy = FsPolicy::new(
        deny_read.iter().map(|path| literal(path)).collect::<Result<_, _>>()?,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![literal("/workspace")?],
        Vec::new(),
    )?;
    Ok(plan_mounts_with(
        &policy,
        &layout,
        &LinuxHostTree,
        &FilesystemOptions::default(),
    )?)
}

#[test]
fn a_deny_through_a_link_to_a_name_that_is_no_text_is_planned() -> Result<(), Box<dyn std::error::Error>> {
    plan_with_non_text_link(&["/workspace/link/file"])?;
    Ok(())
}

#[test]
fn a_policy_that_only_walks_past_a_name_that_is_no_text_is_planned() -> Result<(), Box<dyn std::error::Error>> {
    plan_with_non_text_link(&[])?;
    Ok(())
}
