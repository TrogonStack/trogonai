//! The guest exec path against fake `virtctl` and `kubectl` scripts: the `virtctl guestos exec` path, the
//! `kubectl exec` fallback into the virt-launcher pod, and every way either can fail.

mod guest_support;

use std::path::{Path, PathBuf};

use base64::Engine as _;
use guest_support::{SCRIPTS, TestResult, command_of, invocations, logging, public_key_line, script};
use trogon_sandbox_kubevirt_worker::{DEFAULT_KUBECTL, DEFAULT_VIRTCTL, GuestExec, GuestExecError};

/// `virtctl` that is available and runs `body` for everything but `version`, logging its arguments to `log`.
fn virtctl(dir: &Path, log: &Path, body: &str) -> TestResult<PathBuf> {
    let logged = logging(log);
    script(
        dir,
        "virtctl",
        &format!("case \"$1\" in version) exit 0;; esac\n{logged}\n{body}"),
    )
}

/// `kubectl` that logs its arguments to `log`, answers `get` with `get` and `exec` with `exec`.
fn kubectl(dir: &Path, log: &Path, get: &str, exec: &str) -> TestResult<PathBuf> {
    let logged = logging(log);
    script(
        dir,
        "kubectl",
        &format!("{logged}\ncase \"$1\" in get) {get};; exec) {exec};; esac"),
    )
}

fn missing(dir: &Path, name: &str) -> PathBuf {
    dir.join(name)
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

const LAUNCHER_LOOKUP: &[&str] = &[
    "get",
    "pods",
    "-n",
    "ns",
    "-l",
    "kubevirt.io=virt-launcher,kubevirt.io/domain=vm-1",
    "-o",
    "jsonpath={.items[0].metadata.name}",
];

#[tokio::test]
async fn virtctl_runs_the_command_through_guestos_exec() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "echo guest-output")?,
        kubectl: missing(dir.path(), "kubectl"),
    };

    let output = exec.run("ns", "vm-1", "uname -a").await?;

    assert_eq!(output, "guest-output\n");
    let expected = args(&[
        "guestos",
        "exec",
        "vm-1",
        "--namespace",
        "ns",
        "--",
        "bash",
        "-lc",
        "uname -a",
    ]);
    assert_eq!(invocations(&log)?, [expected]);
    Ok(())
}

#[tokio::test]
async fn a_failing_virtctl_exec_reports_its_stderr() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "echo guest-agent-down >&2; exit 2")?,
        kubectl: missing(dir.path(), "kubectl"),
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(error, Err(GuestExecError::Virtctl { stderr }) if stderr == "guest-agent-down\n"));
    Ok(())
}

#[tokio::test]
async fn a_virtctl_that_vanishes_after_its_version_check_cannot_run() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let virtctl = script(dir.path(), "virtctl", "rm -f \"$0\"")?;
    let exec = GuestExec {
        virtctl: virtctl.clone(),
        kubectl: missing(dir.path(), "kubectl"),
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(error, Err(GuestExecError::Spawn { program, .. }) if program == virtctl));
    Ok(())
}

#[tokio::test]
async fn without_virtctl_the_command_runs_in_the_virt_launcher_pod() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kubectl.log");
    let exec = GuestExec {
        virtctl: missing(dir.path(), "virtctl"),
        kubectl: kubectl(dir.path(), &log, "echo virt-launcher-vm-1-x7k2p", "echo from-launcher")?,
    };

    let output = exec.run("ns", "vm-1", "uname -a").await?;

    assert_eq!(output, "from-launcher\n");
    let exec_args = [
        "exec",
        "-n",
        "ns",
        "virt-launcher-vm-1-x7k2p",
        "-c",
        "compute",
        "--",
        "bash",
        "-lc",
        "uname -a",
    ];
    assert_eq!(invocations(&log)?, [args(LAUNCHER_LOOKUP), args(&exec_args)]);
    Ok(())
}

#[tokio::test]
async fn a_virtctl_whose_version_check_fails_falls_back_to_kubectl() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kubectl.log");
    let exec = GuestExec {
        virtctl: script(dir.path(), "virtctl", "exit 1")?,
        kubectl: kubectl(dir.path(), &log, "echo virt-launcher-vm-1-x7k2p", "echo from-launcher")?,
    };

    let output = exec.run("ns", "vm-1", "true").await?;

    assert_eq!(output, "from-launcher\n");
    assert_eq!(invocations(&log)?.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_failing_launcher_lookup_reports_the_vm_and_stderr() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kubectl.log");
    let exec = GuestExec {
        virtctl: missing(dir.path(), "virtctl"),
        kubectl: kubectl(dir.path(), &log, "echo forbidden >&2; exit 1", "echo unreachable")?,
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(
        error,
        Err(GuestExecError::LauncherLookup { vm_name, stderr }) if vm_name == "vm-1" && stderr == "forbidden\n"
    ));
    assert_eq!(invocations(&log)?, [args(LAUNCHER_LOOKUP)]);
    Ok(())
}

#[tokio::test]
async fn a_vm_without_a_launcher_pod_is_not_found() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kubectl.log");
    let exec = GuestExec {
        virtctl: missing(dir.path(), "virtctl"),
        kubectl: kubectl(dir.path(), &log, "echo '  '", "echo unreachable")?,
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(error, Err(GuestExecError::LauncherNotFound { vm_name }) if vm_name == "vm-1"));
    assert_eq!(invocations(&log)?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn a_failing_kubectl_exec_reports_its_stderr() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kubectl.log");
    let exec = GuestExec {
        virtctl: missing(dir.path(), "virtctl"),
        kubectl: kubectl(
            dir.path(),
            &log,
            "echo virt-launcher-vm-1-x7k2p",
            "echo no-bash >&2; exit 126",
        )?,
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(error, Err(GuestExecError::Kubectl { stderr }) if stderr == "no-bash\n"));
    Ok(())
}

#[tokio::test]
async fn a_kubectl_that_vanishes_after_the_lookup_cannot_exec() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("kubectl.log");
    let kubectl = kubectl(dir.path(), &log, "echo virt-launcher-vm-1-x7k2p; rm -f \"$0\"", "true")?;
    let exec = GuestExec {
        virtctl: missing(dir.path(), "virtctl"),
        kubectl: kubectl.clone(),
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(error, Err(GuestExecError::Spawn { program, .. }) if program == kubectl));
    Ok(())
}

#[tokio::test]
async fn without_either_program_nothing_runs() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let kubectl = missing(dir.path(), "kubectl");
    let exec = GuestExec {
        virtctl: missing(dir.path(), "virtctl"),
        kubectl: kubectl.clone(),
    };

    let error = exec.run("ns", "vm-1", "true").await;

    assert!(matches!(error, Err(GuestExecError::Spawn { program, .. }) if program == kubectl));
    Ok(())
}

#[tokio::test]
async fn an_authorized_key_is_appended_once_with_quotes_escaped() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "true")?,
        kubectl: missing(dir.path(), "kubectl"),
    };
    let key = format!("{} it's", public_key_line()?);
    let escaped = key.replace('\'', "'\\''");

    exec.append_authorized_key("ns", "vm-1", &key).await?;

    let expected = format!(
        "mkdir -p /home/sandbox/.ssh && chmod 700 /home/sandbox/.ssh && grep -qxF '{escaped}' \
         /home/sandbox/.ssh/authorized_keys 2>/dev/null || echo '{escaped}' >> /home/sandbox/.ssh/authorized_keys && \
         chown -R sandbox:sandbox /home/sandbox/.ssh && chmod 600 /home/sandbox/.ssh/authorized_keys"
    );
    let calls = invocations(&log)?;
    assert_eq!(calls.len(), 1);
    assert_eq!(calls.first().and_then(|call| command_of(call)), Some(expected.as_str()));
    Ok(())
}

#[tokio::test]
async fn an_authorized_key_is_removed_by_matching_line() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "true")?,
        kubectl: missing(dir.path(), "kubectl"),
    };
    let key = format!("{} it's", public_key_line()?);
    let escaped = key.replace('\'', "'\\''");

    exec.remove_authorized_key("ns", "vm-1", &key).await?;

    let expected = format!(
        "f=/home/sandbox/.ssh/authorized_keys; \
         if [ -f \"$f\" ]; then \
         grep -vxF '{escaped}' \"$f\" > \"$f.new\"; rc=$?; \
         if [ \"$rc\" -gt 1 ]; then rm -f \"$f.new\"; exit \"$rc\"; fi; \
         cat \"$f.new\" > \"$f\" && rm -f \"$f.new\"; \
         fi"
    );
    let calls = invocations(&log)?;
    assert_eq!(calls.first().and_then(|call| command_of(call)), Some(expected.as_str()));
    Ok(())
}

#[tokio::test]
async fn key_changes_report_a_failing_exec() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "exit 1")?,
        kubectl: missing(dir.path(), "kubectl"),
    };
    let key = public_key_line()?;

    let appended = exec.append_authorized_key("ns", "vm-1", &key).await;
    let removed = exec.remove_authorized_key("ns", "vm-1", &key).await;

    assert!(matches!(appended, Err(GuestExecError::Virtctl { .. })));
    assert!(matches!(removed, Err(GuestExecError::Virtctl { .. })));
    Ok(())
}

#[test]
fn the_default_programs_are_looked_up_on_the_path() {
    let exec = GuestExec::default();

    assert_eq!(exec.virtctl, PathBuf::from(DEFAULT_VIRTCTL));
    assert_eq!(exec.kubectl, PathBuf::from(DEFAULT_KUBECTL));
}

#[test]
fn every_exec_error_describes_itself() {
    let errors = [
        GuestExecError::Spawn {
            program: PathBuf::from("virtctl"),
            source: std::io::Error::other("gone"),
        },
        GuestExecError::Virtctl {
            stderr: "down".to_string(),
        },
        GuestExecError::LauncherLookup {
            vm_name: "vm-1".to_string(),
            stderr: "forbidden".to_string(),
        },
        GuestExecError::LauncherNotFound {
            vm_name: "vm-1".to_string(),
        },
        GuestExecError::Kubectl {
            stderr: "no-bash".to_string(),
        },
    ];
    for error in errors {
        assert!(!error.to_string().is_empty());
    }
}

/// Runs a command the exec path would send to the guest on this machine instead, with the guest's home directory
/// moved to `home` and the ownership change (which needs the guest's account) turned into a no-op.
fn run_locally(command: &str, home: &Path) -> TestResult<std::process::ExitStatus> {
    let local = command
        .replace("/home/sandbox", &home.display().to_string())
        .replace("chown -R sandbox:sandbox", "true");
    Ok(std::process::Command::new("sh").arg("-c").arg(local).status()?)
}

/// The command `exec` sent to the fake `virtctl`, from its log.
fn sent_command(log: &Path) -> TestResult<String> {
    let calls = invocations(log)?;
    let call = calls.last().ok_or("virtctl did not run")?;
    Ok(command_of(call).ok_or("no command")?.to_string())
}

#[tokio::test]
async fn removing_a_key_deletes_only_its_exact_line() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "true")?,
        kubectl: missing(dir.path(), "kubectl"),
    };
    let home = dir.path().join("home");
    let slashes = base64::engine::general_purpose::STANDARD.encode([0xffu8; 33]);
    assert!(slashes.contains('/'));
    let target = format!("ssh-ed25519 {slashes} it's a comment");
    let other = public_key_line()?;
    let longer = format!("{target} and more");

    // Install both keys and a line that only contains the target, through the real install command.
    for key in [&target, &other, &longer] {
        exec.append_authorized_key("ns", "vm-1", key).await?;
        assert!(run_locally(&sent_command(&log)?, &home)?.success());
    }
    let file = home.join(".ssh/authorized_keys");
    assert_eq!(
        std::fs::read_to_string(&file)?,
        format!("{target}\n{other}\n{longer}\n"),
        "install appends the exact full line"
    );
    // Installing again adds nothing, which is what makes `-x` the right match.
    exec.append_authorized_key("ns", "vm-1", &target).await?;
    assert!(run_locally(&sent_command(&log)?, &home)?.success());
    assert_eq!(std::fs::read_to_string(&file)?.lines().count(), 3);

    exec.remove_authorized_key("ns", "vm-1", &target).await?;
    assert!(run_locally(&sent_command(&log)?, &home)?.success());

    assert_eq!(std::fs::read_to_string(&file)?, format!("{other}\n{longer}\n"));
    Ok(())
}

#[tokio::test]
async fn removing_the_only_key_leaves_an_empty_file_and_a_missing_file_alone() -> TestResult {
    let _scripts = SCRIPTS.lock().await;
    let dir = tempfile::tempdir()?;
    let log = dir.path().join("virtctl.log");
    let exec = GuestExec {
        virtctl: virtctl(dir.path(), &log, "true")?,
        kubectl: missing(dir.path(), "kubectl"),
    };
    let home = dir.path().join("home");
    let key = public_key_line()?;
    exec.remove_authorized_key("ns", "vm-1", &key).await?;
    let removal = sent_command(&log)?;

    assert!(
        run_locally(&removal, &home)?.success(),
        "a guest without the file is fine"
    );
    assert!(!home.join(".ssh/authorized_keys").exists());

    std::fs::create_dir_all(home.join(".ssh"))?;
    let file = home.join(".ssh/authorized_keys");
    std::fs::write(&file, format!("{key}\n"))?;
    assert!(run_locally(&removal, &home)?.success());
    assert_eq!(std::fs::read_to_string(&file)?, "");
    assert!(!home.join(".ssh/authorized_keys.new").exists());
    Ok(())
}
