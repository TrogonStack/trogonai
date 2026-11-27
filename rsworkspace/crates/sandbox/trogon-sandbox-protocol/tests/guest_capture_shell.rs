#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

use common::TestResult;
use trogon_sandbox_protocol::{
    SIGNAL_BY_MARKER_SCRIPT, guest_capture_script, retained_guest_capture_cleanup_script,
    retained_guest_capture_read_script, retained_guest_capture_script,
};

fn run_with(timeout_bin: &str, limit: &str, command: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(guest_capture_script(timeout_bin, "env"))
        .arg("sandbox-capture")
        .arg(limit)
        .arg("/")
        .args(command)
        .output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string())
}

fn run_script(limit: &str, command: &[&str]) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Ok(version) = Command::new("timeout").arg("--version").output() else {
        return Ok(None);
    };
    if !String::from_utf8_lossy(&version.stdout).contains("GNU") {
        return Ok(None);
    }
    run_with("timeout", limit, command).map(Some)
}

fn fake_timeout(dir: &tempfile::TempDir, body: &str) -> Result<String, Box<dyn std::error::Error>> {
    let path = dir.path().join("timeout");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path.to_str().ok_or("path")?.to_string())
}

const BUSYBOX_TIMEOUT: &str = r#"k=0; if [ "$1" = -k ]; then k=${2%s}; shift 2; fi
s=${1%s}; shift; me=$$
( sleep "$s"; kill -TERM "$me" 2>/dev/null; sleep "$k"; kill -KILL "$me" 2>/dev/null ) >/dev/null 2>&1 &
exec "$@""#;

#[test]
fn the_script_reports_a_real_exit_124_and_a_real_timeout_apart() -> TestResult {
    let Some(own) = run_script("5s", &["sh", "-c", "exit 124"])? else {
        return Ok(());
    };
    assert_eq!(own, "124");
    assert_eq!(run_script("1s", &["sh", "-c", "sleep 30"])?.as_deref(), Some("timeout"));
    assert_eq!(
        run_script("1s", &["sh", "-c", "trap '' TERM; sleep 30"])?.as_deref(),
        Some("timeout")
    );
    assert_eq!(run_script("5s", &["sh", "-c", "exit 137"])?.as_deref(), Some("137"));
    assert_eq!(run_script("5s", &["sh", "-c", "exit 3"])?.as_deref(), Some("3"));
    Ok(())
}

#[test]
fn the_script_reports_exits_and_timeouts_apart_under_busybox_semantics() -> TestResult {
    let dir = tempfile::tempdir()?;
    let busybox = fake_timeout(&dir, BUSYBOX_TIMEOUT)?;
    assert_eq!(run_with(&busybox, "5s", &["sh", "-c", "exit 124"])?, "124");
    assert_eq!(run_with(&busybox, "5s", &["sh", "-c", "exit 143"])?, "143");
    assert_eq!(run_with(&busybox, "1s", &["sleep", "30"])?, "timeout");
    assert_eq!(
        run_with(&busybox, "1s", &["sh", "-c", "trap '' TERM; sleep 30"])?,
        "timeout"
    );
    Ok(())
}

#[test]
fn a_term_that_lands_before_the_trap_is_still_a_timeout() -> TestResult {
    let dir = tempfile::tempdir()?;
    let early = fake_timeout(&dir, "kill -TERM $$")?;
    assert_eq!(run_with(&early, "5s", &["true"])?, "timeout");
    Ok(())
}

#[test]
fn a_wrapper_that_cannot_start_the_shell_reports_its_own_status() -> TestResult {
    let dir = tempfile::tempdir()?;
    let missing = fake_timeout(&dir, "exit 127")?;
    assert_eq!(run_with(&missing, "5s", &["true"])?, "127");
    Ok(())
}

#[test]
fn every_script_is_valid_shell() -> TestResult {
    let scripts = [
        guest_capture_script("timeout", "env"),
        retained_guest_capture_script("timeout", "env"),
        retained_guest_capture_read_script(),
        retained_guest_capture_cleanup_script(),
        SIGNAL_BY_MARKER_SCRIPT.to_string(),
    ];
    for script in scripts {
        let status = Command::new("sh").arg("-n").arg("-c").arg(&script).status()?;
        assert!(status.success(), "{script}");
    }
    Ok(())
}

#[test]
fn the_signal_script_exits_nonzero_when_nothing_carries_the_marker() -> TestResult {
    let marker = format!("TROGON_TEST_MARKER=absent-{}", std::process::id());
    let status = Command::new("sh")
        .args(["-c", SIGNAL_BY_MARKER_SCRIPT, "sh", &marker, "TERM"])
        .stderr(Stdio::null())
        .status()?;
    assert!(!status.success());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn marker_signal_reaches_a_child_forked_by_the_signal_handler() -> TestResult {
    let marker = format!("TROGON_TEST_MARKER=fork-{}", std::process::id());
    let (key, value) = marker.split_once('=').ok_or("marker")?;
    let mut spawner = Command::new("sh")
        .args(["-c", "trap 'sleep 60 & exit 0' TERM; while :; do sleep 0.1; done"])
        .env(key, value)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    std::thread::sleep(std::time::Duration::from_millis(50));
    let status = Command::new("sh")
        .args(["-c", SIGNAL_BY_MARKER_SCRIPT, "sh", &marker, "TERM"])
        .stderr(Stdio::null())
        .status()?;
    let _ = spawner.wait();
    let survivors = std::fs::read_dir("/proc")?
        .flatten()
        .filter(|entry| {
            std::fs::read(entry.path().join("environ"))
                .map(|environ| environ.split(|byte| *byte == 0).any(|line| line == marker.as_bytes()))
                .unwrap_or(false)
        })
        .filter(|entry| {
            std::fs::read_to_string(entry.path().join("stat"))
                .map(|stat| !stat.contains(") Z "))
                .unwrap_or(false)
        })
        .count();
    assert!(status.success());
    assert_eq!(survivors, 0, "processes carrying the marker survived");
    Ok(())
}
