#![cfg(target_os = "linux")]

use std::path::PathBuf;

use trogon_sandbox_contract::{LinuxOptions, PlatformOptions};
use trogon_sandbox_subprocess_worker::isolation::limits::ResourceLimits;
use trogon_sandbox_subprocess_worker::isolation::{BubblewrapBackend, IsolationBackend};
use trogon_sandbox_subprocess_worker::{EgressPolicy, LaunchSpec};

const PROBE: &str = r#"
import ctypes, errno, os, socket, sys, threading

sys.stdout = open("/workspace/probe.txt", "w")

def errno_of(fn):
    try:
        fn()
        return "ok"
    except OSError as e:
        return errno.errorcode[e.errno]

print("unix", errno_of(lambda: socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)))
print("vsock", errno_of(lambda: socket.socket(40, socket.SOCK_STREAM)))
print("inet", errno_of(lambda: socket.socket(socket.AF_INET, socket.SOCK_STREAM)))
print("socketpair", errno_of(lambda: socket.socketpair(socket.AF_UNIX)))
libc = ctypes.CDLL(None, use_errno=True)
CLONE_NEWUSER = 0x10000000
SIGCHLD = 17
x86 = os.uname().machine == "x86_64"
rc = libc.syscall(272 if x86 else 97, CLONE_NEWUSER)
print("unshare", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
rc = libc.syscall(56 if x86 else 220, CLONE_NEWUSER | SIGCHLD, 0, 0, 0, 0)
print("clone_newuser", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
rc = libc.syscall(435, 0, 0)
print("clone3", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
rc = libc.syscall(442, -1, b"", 0, None, 0)
print("mount_setattr", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
rc = libc.syscall(467, -100, b"/", 0, None, 0)
print("open_tree_attr", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
CLONE_NEWNS = 0x00020000
rc = libc.unshare(CLONE_NEWNS)
print("unshare_newns", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
rc = libc.syscall(56 if x86 else 220, CLONE_NEWNS | SIGCHLD, 0, 0, 0, 0)
if rc == 0:
    os._exit(0)
print("clone_newns", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
CLONE_NEWUTS = 0x04000000
rc = libc.syscall(272 if x86 else 97, CLONE_NEWUTS)
print("unshare_newuts", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
rc = libc.syscall(425, 1, ctypes.create_string_buffer(120))
print("io_uring", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
word = ctypes.create_string_buffer(8)
iov = (ctypes.c_void_p * 2)(ctypes.addressof(word), 8)
rc = libc.syscall(311 if x86 else 271, os.getpid(), iov, 1, iov, 1, 0)
print("process_vm_writev", "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])
pid = os.fork()
if pid == 0:
    os._exit(0 if libc.syscall(101 if x86 else 117, 0, 0, 0, 0) == 0 else 1)
print("ptrace_traceme", "ok" if os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]) == 0 else "FAIL")
for label, target in (("root", b"/"), ("workspace", b"/workspace"), ("proc", b"/proc")):
    rc = libc.umount2(target, 0)
    print("umount_" + label, "ok" if rc == 0 else errno.errorcode[ctypes.get_errno()])
def said(label, rc):
    print(label, "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])

NEWUSER = 0x10000000
said("unshare_high_bits", libc.syscall(272 if x86 else 97, ctypes.c_ulong(NEWUSER | (1 << 40))))
said("unshare_empty_mntns", libc.unshare(0x00100000))
said("setns", libc.syscall(308 if x86 else 268, -1, 0))
said("open_tree", libc.syscall(428, -100, b"/", 0))
said("fsconfig", libc.syscall(431, -1, 0, None, None, 0))
said("mount", libc.mount(None, None, None, 0, None))
said("bpf", libc.syscall(321 if x86 else 280, 0, 0, 0))
said("userfaultfd", libc.syscall(323 if x86 else 282, 1))
pid = os.fork()
if pid == 0:
    os._exit(0)
said("fork", os.waitpid(pid, 0)[1])
print("sid", os.getsid(0))
print("pgid", os.getpgid(0))
print("devtty", errno_of(lambda: os.close(os.open("/dev/tty", os.O_RDWR))))
def tiocsti():
    import fcntl, termios
    fcntl.ioctl(0, termios.TIOCSTI, b"x")
print("tiocsti", errno_of(tiocsti))
with open("/proc/self/status") as status:
    caps = dict(line.split(":\t", 1) for line in status if ":\t" in line)
print("capeff", caps["CapEff"].strip())
print("capbnd", caps["CapBnd"].strip())

done = []
t = threading.Thread(target=lambda: done.append(1))
t.start(); t.join()
print("thread", "ok" if done else "failed")
print("env", ",".join(sorted(os.environ)))
"#;

const NESTED_PROBE: &str = r#"
import ctypes, errno, os, socket, sys

sys.stdout = open("/workspace/probe.txt", "w")
libc = ctypes.CDLL(None, use_errno=True)

def said(label, rc):
    print(label, "ok" if rc >= 0 else errno.errorcode[ctypes.get_errno()])

x86 = os.uname().machine == "x86_64"
said("setns", libc.syscall(308 if x86 else 268, -1, 0))
said("open_tree", libc.syscall(428, -100, b"/", 0))
said("fsconfig", libc.syscall(431, -1, 0, None, None, 0))
try:
    socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    print("unix ok")
except OSError as e:
    print("unix", errno.errorcode[e.errno])
"#;

fn running_as_root() -> bool {
    nix::unistd::geteuid().is_root()
}

fn tools_available() -> bool {
    which::which("bwrap").is_ok() && which::which("python3").is_ok()
}

async fn run_probe(
    env: Vec<(String, String)>,
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error>> {
    run_probe_with(env, Default::default()).await
}

async fn run_probe_with(
    env: Vec<(String, String)>,
    platform: PlatformOptions,
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error>> {
    run_script(PROBE, env, platform).await
}

async fn run_script(
    script: &str,
    env: Vec<(String, String)>,
    platform: PlatformOptions,
) -> Result<std::collections::BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let scratch = tempfile::tempdir()?;
    let backend = BubblewrapBackend::new(which::which("bwrap")?)
        .with_guest_init(PathBuf::from(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker")));
    let spec = LaunchSpec {
        binary: which::which("python3")?,
        args: vec!["-c".to_string(), script.to_string()],
        env,
        workspace_host_path: scratch.path().to_path_buf(),
        workspace_sandbox_path: PathBuf::from("/workspace"),
        workspace_readonly: false,
        egress_policy: EgressPolicy::DenyAll,
        extra_binds: vec![],
        user: None,
        limits: ResourceLimits::default(),
        fs_policy: None,
        platform,
        fs_denials: None,
    };

    let launched = backend.launch(&spec).await?;
    let child = launched.child.lock().await.take().ok_or("child handle")?;
    let status = child.wait_with_output().await?.status;
    let report = std::fs::read_to_string(scratch.path().join("probe.txt")).unwrap_or_default();
    assert!(status.success(), "probe exited with {status}: {report}");
    Ok(report
        .lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect())
}

#[tokio::test]
async fn seccomp_blocks_host_reaching_sockets_and_namespaces() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    assert_eq!(results["unix"], "EPERM");
    assert_eq!(results["vsock"], "EPERM");
    assert_eq!(results["inet"], "ok");
    assert_eq!(results["socketpair"], "ok");
    assert_eq!(results["unshare"], "EPERM");
    assert_eq!(results["clone_newuser"], "EPERM");
    assert_eq!(results["clone3"], "ENOSYS");
    assert_eq!(results["thread"], "ok");
    Ok(())
}

#[tokio::test]
async fn seccomp_refuses_only_the_namespace_io_uring_and_socket_calls() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    assert_eq!(results["io_uring"], "EPERM");
    assert_eq!(results["process_vm_writev"], "ok");
    assert_eq!(results["ptrace_traceme"], "ok");
    assert_eq!(
        results["unshare_newuts"],
        if running_as_root() { "ok" } else { "EPERM" },
        "refused for want of a capability"
    );
    Ok(())
}

#[tokio::test]
async fn seccomp_denies_the_mount_attribute_calls() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    assert_eq!(results["mount_setattr"], "EPERM");
    assert_eq!(results["open_tree_attr"], "EPERM");
    Ok(())
}

#[tokio::test]
async fn the_payload_cannot_unmount_anything_or_make_a_mount_namespace() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    assert_eq!(results["unshare_newns"], "EPERM");
    assert_eq!(results["clone_newns"], "EPERM");
    assert_eq!(results["umount_root"], "EPERM");
    assert_eq!(results["umount_workspace"], "EPERM");
    assert_eq!(results["umount_proc"], "EPERM");
    Ok(())
}

#[tokio::test]
async fn payload_runs_in_its_own_session_with_no_capabilities() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    let parent_sid = unsafe { nix::libc::getsid(0) };
    let sid: i32 = results["sid"].parse()?;
    let pgid: i32 = results["pgid"].parse()?;
    assert_ne!(sid, parent_sid, "the payload shares the worker's session");
    assert_ne!(pgid, unsafe { nix::libc::getpgid(0) });
    assert_ne!(results["devtty"], "ok", "the payload has a controlling tty");
    assert_eq!(results["tiocsti"], "EPERM", "TIOCSTI was not refused");
    let expected = if running_as_root() {
        "000001ffffffffff"
    } else {
        "0000000000000000"
    };
    assert_eq!(results["capeff"], expected);
    assert_eq!(results["capbnd"], expected);
    Ok(())
}

#[tokio::test]
async fn sandbox_does_not_inherit_worker_environment() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    // SAFETY: test process sets a variable before spawning; no other test reads it.
    unsafe { std::env::set_var("SANDBOX_WORKER_ONLY_SECRET", "leak") };
    let results = run_probe(vec![("LEASE_VAR".to_string(), "1".to_string())]).await?;

    let env: Vec<&str> = results["env"].split(',').collect();
    assert!(!env.contains(&"SANDBOX_WORKER_ONLY_SECRET"));
    assert!(env.contains(&"LEASE_VAR"));
    assert!(env.contains(&"PATH"));
    Ok(())
}

#[tokio::test]
async fn the_namespace_calls_are_answered_by_the_filter_not_by_the_kernel() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    for call in [
        "unshare_high_bits",
        "unshare_empty_mntns",
        "setns",
        "open_tree",
        "fsconfig",
        "mount",
    ] {
        assert_eq!(results[call], "EPERM", "{call}: {results:?}");
    }
    assert_eq!(results["fork"], "ok");
    Ok(())
}

#[tokio::test]
async fn bpf_and_userfaultfd_get_the_kernel_answer() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe(vec![]).await?;

    if running_as_root() {
        assert_eq!(results["bpf"], "EINVAL");
        assert_eq!(results["userfaultfd"], "ok");
    } else {
        assert_eq!(results["bpf"], "EPERM");
        assert_eq!(results["userfaultfd"], "EPERM");
    }
    Ok(())
}

#[tokio::test]
async fn allowing_unix_sockets_lifts_only_the_socket_rule() -> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_probe_with(
        vec![],
        PlatformOptions {
            linux: LinuxOptions {
                allow_all_unix_sockets: true.into(),
                ..LinuxOptions::default()
            },
            ..PlatformOptions::default()
        },
    )
    .await?;

    assert_eq!(results["unix"], "ok");
    assert_eq!(results["vsock"], "ok");
    assert_eq!(results["unshare"], "ENOSPC");
    assert_eq!(results["clone_newuser"], "ENOSPC");
    assert_eq!(results["unshare_newns"], "EPERM");
    assert_eq!(results["setns"], "EBADF");
    assert_eq!(results["capeff"], "0000000000000000");
    assert_eq!(results["capbnd"], "0000000000000000");
    Ok(())
}

#[tokio::test]
async fn allowing_nested_namespaces_lifts_the_namespace_rules_and_keeps_the_socket_rule()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_script(
        NESTED_PROBE,
        vec![],
        PlatformOptions {
            linux: LinuxOptions {
                allow_nested_user_namespaces: true.into(),
                ..LinuxOptions::default()
            },
            ..PlatformOptions::default()
        },
    )
    .await?;

    assert_eq!(results["setns"], "EBADF");
    assert_eq!(results["open_tree"], "ok");
    assert_eq!(results["fsconfig"], "EINVAL");
    assert_eq!(results["unix"], "EPERM");
    Ok(())
}

const REACH_PROBE: &str = r#"
import ctypes, errno, os
libc = ctypes.CDLL(None, use_errno=True)
out = open("/workspace/probe.txt", "w")
pids = sorted(int(p) for p in os.listdir("/proc") if p.isdigit())
print("pids", ",".join(map(str, pids)), file=out)
print("self", os.getpid(), file=out)
for p in pids:
    print("cmd%d" % p, open("/proc/%d/cmdline" % p).read().replace("\0", "_")[:40].replace(" ", "~"), file=out)
def ptrace(pid):
    ctypes.set_errno(0)
    libc.ptrace(16, pid, 0, 0)
    return errno.errorcode.get(ctypes.get_errno(), "ok")
print("ptrace_missing", ptrace(1 << 20), file=out)
out.close()
"#;

#[tokio::test]
async fn the_payload_runs_in_a_pid_namespace_of_its_own_and_cannot_see_the_relay_or_init()
-> Result<(), Box<dyn std::error::Error>> {
    if !tools_available() {
        return Ok(());
    }
    let results = run_script(REACH_PROBE, vec![], PlatformOptions::default()).await?;

    let own: u32 = results["self"].parse()?;
    assert_eq!(own, 2, "the payload is not the nested init's only child: {results:?}");
    assert_eq!(results["ptrace_missing"], "ESRCH");
    // A container that masks /proc keeps the outer mount, as it does,
    // so the listing only narrows where a fresh one could be mounted.
    if !results["cmd1"].contains("--guest-init") {
        assert_eq!(
            results["pids"], "1,2",
            "the payload sees outside processes: {results:?}"
        );
    }
    Ok(())
}
