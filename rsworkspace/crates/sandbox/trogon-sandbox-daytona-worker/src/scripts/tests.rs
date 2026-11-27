use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn request(change: impl FnOnce(&mut Value)) -> Result<RunCommandRequest, serde_json::Error> {
    let mut wire = json!({
        "lease_id": "lease-1",
        "command": ["echo", "it's"],
        "timeout_seconds": 30,
        "env": {"B": "2", "A": "1"}
    });
    change(&mut wire);
    serde_json::from_value(wire)
}

fn sh(script: &str) -> Result<(i32, String), Box<dyn std::error::Error>> {
    let output = Command::new("/bin/sh").arg("-c").arg(script).output()?;
    Ok((output.status.code().unwrap_or(-1), String::from_utf8(output.stdout)?))
}

#[test]
fn the_capture_words_carry_the_limit_the_directory_the_environment_and_the_command() -> TestResult {
    let words = capture_words(&request(|_| {})?, CommandDeadline::Timeout, None);
    assert_eq!(&words[..2], ["sh", "-c"]);
    assert_eq!(
        &words[3..],
        ["sandbox-capture", "30s", "/workspace", "A=1", "B=2", "echo", "it's"]
    );
    assert_eq!(words[2], guest_capture_script("timeout", "env"));
    Ok(())
}

#[test]
fn a_lease_bound_command_runs_without_a_deadline() -> TestResult {
    let words = capture_words(&request(|_| {})?, CommandDeadline::Lease, None);
    assert!(words[2].starts_with("capture_without_deadline() { shift 3;"));
    assert!(words[2].contains(&guest_capture_script("capture_without_deadline", "env")));
    Ok(())
}

#[test]
fn a_relative_directory_joins_the_default_and_the_marker_tags_the_command() -> TestResult {
    let words = capture_words(
        &request(|wire| wire["cwd"] = json!("sub"))?,
        CommandDeadline::Timeout,
        Some("abc"),
    );
    assert_eq!(words[5], "/workspace/sub");
    assert_eq!(words[8], "SANDBOX_EXECUTION_ID=abc");
    Ok(())
}

#[test]
fn another_user_runs_through_sudo_from_their_home() -> TestResult {
    let words = capture_words(
        &request(|wire| wire["run_as_user"] = json!("builder"))?,
        CommandDeadline::Timeout,
        None,
    );
    assert_eq!(words[5], "/home/builder");
    assert_eq!(
        &words[6..14],
        ["sudo", "-n", "-u", "builder", "env", "A=1", "B=2", "echo"]
    );
    let root = capture_words(
        &request(|wire| wire["run_as_user"] = json!("root"))?,
        CommandDeadline::Timeout,
        None,
    );
    assert_eq!(root[5], "/workspace");
    Ok(())
}

#[test]
fn quoted_words_reach_the_shell_intact() -> TestResult {
    let words = vec!["it's".to_owned(), "a b".to_owned(), "$HOME".to_owned()];
    let (code, out) = sh(&format!("printf '%s|' {}", join_quoted(&words)))?;
    assert_eq!((code, out.as_str()), (0, "it's|a b|$HOME|"));
    Ok(())
}

#[test]
fn a_detached_command_reports_running_until_its_done_marker_appears() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("cmd").display().to_string();
    let poll = poll_script(&dir, 3);
    let (code, out) = sh(&detach_command(&dir, "sh -c 'printf frame; exit 7'"))?;
    assert_eq!((code, out.as_str()), (0, ""));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (code, out) = sh(&poll)?;
        if code == 0 {
            assert_eq!(out, "frame");
            break;
        }
        assert_eq!(code, 3);
        assert!(Instant::now() < deadline, "the detached command never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(std::fs::read_to_string(format!("{dir}/done"))?, "7\n");
    let (code, _) = sh(&remove_script(&dir))?;
    assert_eq!(code, 0);
    assert!(!std::path::Path::new(&dir).exists());
    Ok(())
}

#[test]
fn a_poll_of_a_directory_that_was_never_made_reports_running() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let dir = tmp.path().join("never").display().to_string();
    assert_eq!(sh(&poll_script(&dir, 3))?.0, 3);
    Ok(())
}

#[test]
fn the_directory_and_the_marker_name_the_execution() {
    assert_eq!(command_dir("abc"), "/tmp/.trogon-exec-abc");
    assert_eq!(marker_line("abc"), "SANDBOX_EXECUTION_ID=abc");
}

#[cfg(target_os = "linux")]
#[test]
fn the_signal_script_ends_the_process_that_carries_the_marker() -> TestResult {
    let marker = format!("SANDBOX_EXECUTION_ID={}", uuid::Uuid::new_v4().simple());
    let mut child = Command::new("sleep")
        .arg("60")
        .env(
            "SANDBOX_EXECUTION_ID",
            marker.trim_start_matches("SANDBOX_EXECUTION_ID="),
        )
        .spawn()?;
    let (code, _) = sh(&signal_script(&marker, 15))?;
    assert_eq!(code, 0);
    let status = child.wait()?;
    assert!(!status.success());
    let (code, _) = sh(&signal_script(&marker, 15))?;
    assert_eq!(code, 1);
    Ok(())
}
