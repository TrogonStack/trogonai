use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use trogon_sandbox_contract::{StdinChunk, StdinEof};

use super::*;
use crate::constants::COMMAND_FILE_PREFIX;
use crate::side_files::SideFiles;

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

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn a_command_is_rendered_with_every_word_quoted_and_the_directory_first() {
    let line = render_command(&words(&["echo", "it's"]), Some("/work dir"), &[("A", "1 2")]);
    assert_eq!(line, "cd '/work dir' && env A='1 2' 'echo' 'it'\\''s'");
}

#[test]
fn a_rendered_command_runs_back_as_the_same_words() -> TestResult {
    let line = render_command(&words(&["printf", "%s|", "a b", "$HOME", "it's"]), Some("/tmp"), &[]);
    assert_eq!(sh(&line)?, (0, "a b|$HOME|it's|".to_owned()));
    Ok(())
}

#[test]
fn the_environment_reaches_the_command() -> TestResult {
    let line = render_command(&words(&["sh", "-c", "printf %s \"$A\""]), None, &[("A", "x y")]);
    assert_eq!(sh(&line)?, (0, "x y".to_owned()));
    Ok(())
}

#[test]
fn home_directories_follow_the_user() {
    assert_eq!(home_of("root"), "/root");
    assert_eq!(home_of("dev"), "/home/dev");
}

#[test]
fn a_per_call_user_switches_with_su_for_root_and_sudo_otherwise() {
    let line = render_as_user("dev", "echo hi");
    assert!(line.contains("su -s /bin/sh 'dev' -c 'echo hi'"));
    assert!(line.contains("sudo -n -u 'dev' /bin/sh -c 'echo hi'"));
}

#[test]
fn a_request_starts_in_the_workspace_unless_it_names_a_directory() -> TestResult {
    let line = render_request(&request(|_| {})?);

    assert!(line.starts_with(&format!("cd '{WORKSPACE_PATH}'")));
    let line = render_request(&request(|wire| wire["cwd"] = json!("sub"))?);
    assert!(line.starts_with(&format!("cd '{WORKSPACE_PATH}/sub'")));
    Ok(())
}

#[test]
fn a_per_call_user_starts_in_its_home() -> TestResult {
    let line = render_request(&request(|wire| wire["run_as_user"] = json!("dev"))?);
    assert!(line.contains("/home/dev"));
    Ok(())
}

#[test]
fn output_capture_sends_each_stream_to_its_file() -> TestResult {
    let dir = tempfile::tempdir()?;
    let (out, err) = (dir.path().join("o"), dir.path().join("e"));
    let line = capture_output("echo a; echo b >&2", &out.to_string_lossy(), &err.to_string_lossy());
    assert_eq!(sh(&line)?.0, 0);
    assert_eq!(std::fs::read_to_string(out)?, "a\n");
    assert_eq!(std::fs::read_to_string(err)?, "b\n");
    Ok(())
}

fn have_timeout() -> bool {
    Command::new("/bin/sh")
        .arg("-c")
        .arg("command -v timeout >/dev/null 2>&1")
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn a_command_that_ends_in_time_leaves_a_completed_receipt() -> TestResult {
    if !have_timeout() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let status = dir.path().join("s").to_string_lossy().into_owned();
    let line = with_deadline("exit 3", 5, &status, StdinMode::Closed);
    assert_eq!(sh(&line)?.0, 3);
    assert_eq!(std::fs::read_to_string(&status)?, "completed");
    Ok(())
}

#[test]
fn a_command_that_outlives_the_deadline_leaves_a_timed_out_receipt() -> TestResult {
    if !have_timeout() {
        return Ok(());
    }
    let dir = tempfile::tempdir()?;
    let status = dir.path().join("s").to_string_lossy().into_owned();
    let line = with_deadline("sleep 30", 1, &status, StdinMode::Closed);
    let started = Instant::now();
    sh(&line)?;
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(std::fs::read_to_string(&status)?, "timed_out");
    Ok(())
}

#[test]
fn an_image_without_timeout_leaves_an_unavailable_receipt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let status = dir.path().join("s").to_string_lossy().into_owned();
    let line = with_deadline("echo hi", 5, &status, StdinMode::Closed);
    let output = Command::new("/bin/sh").arg("-c").arg(&line).env("PATH", "").output()?;
    assert_eq!(output.status.code(), Some(125));
    assert_eq!(std::fs::read_to_string(&status)?, "unavailable");
    Ok(())
}

#[test]
fn a_closed_stdin_reads_nothing() -> TestResult {
    assert_eq!(sh(&closed_stdin("cat; echo done"))?, (0, "done\n".to_owned()));
    Ok(())
}

#[test]
fn stdin_lines_carry_the_data_as_base64_then_the_end_marker() -> TestResult {
    let write = StdinWrite {
        data: StdinChunk::new(b"hi\n".to_vec())?,
        eof: StdinEof::Close,
    };
    assert_eq!(stdin_lines(&write), ["aGkK", "."]);
    let empty = StdinWrite {
        data: StdinChunk::new(Vec::new())?,
        eof: StdinEof::Keep,
    };
    assert!(stdin_lines(&empty).is_empty());
    Ok(())
}

#[test]
fn the_side_files_of_one_command_share_a_prefix() {
    let files = SideFiles::new("abc");
    assert_eq!(files.out, format!("{COMMAND_FILE_PREFIX}abc.out"));
    assert_eq!(files.all().len(), 5);
    assert!(files.all().iter().all(|path| path.starts_with(COMMAND_FILE_PREFIX)));
}
