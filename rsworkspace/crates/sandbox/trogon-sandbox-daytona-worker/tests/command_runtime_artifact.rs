//! Loading the supervisor artifact an operator configures: every check runs before any sandbox is touched.

mod guest_support;

use std::os::unix::fs::FileTypeExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use guest_support::{Boxed, Guest, TestResult, run_request};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use trogon_sandbox_contract::{CommandExecution, RunCommandResponse};
use trogon_sandbox_daytona_worker::OrdinaryCommandRuntime;
use trogon_std::env::{ReadEnv, SystemEnv};

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn operator_file(bytes: &[u8]) -> Result<(TempDir, PathBuf, String), Boxed> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("supervisor");
    std::fs::write(&path, bytes)?;
    Ok((directory, path, sha256(bytes)))
}

fn refusal(path: &Path, pin: &str) -> Result<String, Boxed> {
    match OrdinaryCommandRuntime::load(path, pin) {
        Ok(_) => Err("the operator artifact must be refused".into()),
        Err(error) => Ok(error.to_string()),
    }
}

fn elf64_headers(machine: u16) -> Vec<u8> {
    let mut bytes = vec![0; 120];
    bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
    bytes[32..40].copy_from_slice(&64_u64.to_le_bytes());
    bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
    bytes[54..56].copy_from_slice(&56_u16.to_le_bytes());
    bytes[56..58].copy_from_slice(&1_u16.to_le_bytes());
    bytes[64..68].copy_from_slice(&1_u32.to_le_bytes());
    bytes
}

#[test]
fn operator_pin_must_be_a_lowercase_sha256_before_opening_a_file() -> TestResult {
    let directory = tempfile::tempdir()?;
    let missing = directory.path().join("missing");
    for pin in [String::new(), "0".repeat(63), "A".repeat(64)] {
        assert!(refusal(&missing, &pin)?.contains("lowercase SHA256 pin"));
    }
    Ok(())
}

#[test]
fn missing_operator_artifact_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    assert!(refusal(&directory.path().join("missing"), &"0".repeat(64))?.contains("open ordinary command artifact"));
    Ok(())
}

#[test]
fn operator_pin_must_match_the_exact_file_bytes() -> TestResult {
    let (_directory, path, pin) = operator_file(&elf64_headers(62))?;
    let mut other = pin.clone();
    other.replace_range(0..1, if pin.starts_with('0') { "1" } else { "0" });
    assert!(refusal(&path, &other)?.contains("does not match its configured SHA256 pin"));
    assert!(OrdinaryCommandRuntime::load(&path, &pin).is_ok());
    Ok(())
}

#[test]
fn an_operator_directory_is_not_a_regular_artifact() -> TestResult {
    let directory = tempfile::tempdir()?;
    assert!(refusal(directory.path(), &"0".repeat(64))?.contains("bounded regular file"));
    Ok(())
}

#[test]
fn an_operator_symlink_is_refused_before_reading_its_target() -> TestResult {
    let (directory, path, pin) = operator_file(&elf64_headers(62))?;
    let link = directory.path().join("supervisor-link");
    std::os::unix::fs::symlink(&path, &link)?;
    assert!(refusal(&link, &pin)?.contains("bounded regular file"));
    Ok(())
}

#[test]
fn fifo_operator_artifact_refuses_without_waiting_for_a_writer() -> TestResult {
    const CHILD_FIFO: &str = "DAYTONA_ARTIFACT_FIFO_CHILD";
    if let Some(path) = SystemEnv.var_os(CHILD_FIFO) {
        let path = PathBuf::from(path);
        std::fs::write(path.with_extension("entered"), b"public-load-entered")?;
        let error = refusal(&path, &"0".repeat(64))?;
        assert!(error.contains("bounded regular file"), "{error}");
        return Ok(());
    }

    let directory = tempfile::tempdir()?;
    let fifo = directory.path().join("supervisor-fifo");
    let created = Command::new("mkfifo").args(["-m", "600"]).arg(&fifo).status()?;
    assert!(created.success());
    assert!(std::fs::symlink_metadata(&fifo)?.file_type().is_fifo());
    let mut child = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "fifo_operator_artifact_refuses_without_waiting_for_a_writer",
            "--nocapture",
        ])
        .env(CHILD_FIFO, &fifo)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut timed_out = false;
    loop {
        if child.try_wait()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output()?;
    let entered = std::fs::read(fifo.with_extension("entered"));
    std::fs::remove_file(&fifo)?;

    assert_eq!(entered?, b"public-load-entered");
    assert!(
        !timed_out,
        "loading blocked on a FIFO without a writer; the child was killed and the FIFO unlinked"
    );
    assert!(
        output.status.success(),
        "loader child failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn an_operator_artifact_larger_than_the_bound_is_refused() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("oversized");
    std::fs::File::create(&path)?.set_len(16 * 1024 * 1024 + 1)?;
    assert!(refusal(&path, &"0".repeat(64))?.contains("bounded regular file"));
    Ok(())
}

#[test]
fn a_correct_pin_does_not_authorize_non_elf_bytes() -> TestResult {
    let (_directory, path, pin) = operator_file(b"#!/bin/sh\nexit 0\n")?;
    assert!(refusal(&path, &pin)?.contains("requires a Linux ELF64 artifact"));
    Ok(())
}

#[test]
fn a_correct_pin_does_not_authorize_an_unsupported_elf_architecture() -> TestResult {
    let (_directory, path, pin) = operator_file(&elf64_headers(243))?;
    assert!(refusal(&path, &pin)?.contains("architecture is unsupported"));
    Ok(())
}

#[test]
fn a_correct_pin_does_not_authorize_a_guest_interpreter() -> TestResult {
    let mut bytes = elf64_headers(62);
    bytes[64..68].copy_from_slice(&3_u32.to_le_bytes());
    let (_directory, path, pin) = operator_file(&bytes)?;
    assert!(refusal(&path, &pin)?.contains("must not use a guest interpreter"));
    Ok(())
}

#[test]
fn a_correct_pin_does_not_authorize_a_truncated_program_table() -> TestResult {
    let mut bytes = elf64_headers(183);
    bytes.truncate(119);
    let (_directory, path, pin) = operator_file(&bytes)?;
    assert!(refusal(&path, &pin)?.contains("artifact is truncated"));
    Ok(())
}

#[tokio::test]
async fn an_artifact_that_cannot_run_in_the_guest_stops_a_detached_command_before_it_starts() -> TestResult {
    let (_directory, path, pin) = operator_file(&elf64_headers(62))?;
    let guest = Guest::start().await?;
    let exec = guest.exec()?.with_runtime(OrdinaryCommandRuntime::load(&path, &pin)?);
    let request = run_request(&["true"], |wire| wire["detached"] = serde_json::json!(true))?;
    assert!(matches!(request.execution, CommandExecution::Detached { .. }));
    let response = exec.handle_run_command(&guest.sandbox(), &request).await;
    // The artifact is not a working supervisor, so the install is refused whatever the host is.
    assert!(matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    assert!(guest.session_commands().is_empty());
    Ok(())
}
