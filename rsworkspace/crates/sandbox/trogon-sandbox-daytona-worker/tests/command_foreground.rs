//! Attached commands against a guest that runs them on this machine.

mod guest_support;

use std::time::{Duration, Instant};

use guest_support::*;
use serde_json::json;
use trogon_sandbox_contract::{CommandOutput, CommandTimeoutStatus, OutputTruncation, RunCommandResponse};

async fn run(guest: &Guest, request: &trogon_sandbox_contract::RunCommandRequest) -> Result<CommandOutput, Boxed> {
    completed(within(guest.exec()?.handle_run_command(&guest.sandbox(), request)).await?)
}

#[tokio::test]
async fn the_streams_and_the_exit_code_come_back_separately() -> TestResult {
    let guest = Guest::start().await?;
    let request = run_request(&["sh", "-c", "echo out; echo warn >&2; exit 7"], |_| {})?;
    let output = run(&guest, &request).await?;
    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, b"out\n");
    assert_eq!(output.stderr, b"warn\n");
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    Ok(())
}

#[tokio::test]
async fn binary_and_nul_bytes_survive_the_round_trip() -> TestResult {
    let guest = Guest::start().await?;
    let request = run_request(&["sh", "-c", "printf 'a\\000b\\377\\n'; printf '\\000e' >&2"], |_| {})?;
    let output = run(&guest, &request).await?;
    assert_eq!(output.stdout, b"a\0b\xff\n");
    assert_eq!(output.stderr, b"\0e");
    Ok(())
}

#[tokio::test]
async fn the_working_directory_and_environment_apply() -> TestResult {
    let guest = Guest::start().await?;
    let request = run_request(&["sh", "-c", "pwd; printf %s \"$GREETING\""], |wire| {
        wire["env"] = json!({"GREETING": "hello there"});
    })?;
    let output = run(&guest, &request).await?;
    assert!(String::from_utf8(output.stdout)?.ends_with("hello there"));
    Ok(())
}

#[tokio::test]
async fn a_command_past_its_deadline_reports_a_timeout() -> TestResult {
    let guest = Guest::start().await?;
    let request = run_request(&["sleep", "30"], |wire| wire["timeout_seconds"] = json!(1))?;
    let started = Instant::now();
    let output = run(&guest, &request).await?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    assert_eq!(output.exit_code, CommandOutput::TIMED_OUT_EXIT_CODE);
    assert!(started.elapsed() < Duration::from_secs(25));
    Ok(())
}

#[tokio::test]
async fn a_zero_deadline_still_gives_the_command_a_second() -> TestResult {
    let guest = Guest::start().await?;
    let quick = run_request(&["echo", "fast"], |wire| wire["timeout_seconds"] = json!(0))?;
    let output = run(&guest, &quick).await?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    assert_eq!(output.stdout, b"fast\n");
    let slow = run_request(&["sleep", "30"], |wire| wire["timeout_seconds"] = json!(0))?;
    let output = run(&guest, &slow).await?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    Ok(())
}

#[tokio::test]
async fn output_beyond_the_cap_is_cut_and_marked() -> TestResult {
    let guest = Guest::start().await?;
    let request = run_request(
        &["sh", "-c", "head -c 5000 /dev/zero | tr '\\0' x; echo e >&2"],
        |wire| {
            wire["max_output_bytes"] = json!(100);
        },
    )?;
    let output = run(&guest, &request).await?;
    assert!(output.stdout.len() < 1000 && output.stdout.starts_with(b"xxxx"));
    assert_eq!(output.stderr, b"e\n");
    assert_eq!(output.truncated, OutputTruncation::Truncated);
    Ok(())
}

#[tokio::test]
async fn what_the_command_left_behind_is_removed() -> TestResult {
    let guest = Guest::start().await?;
    run(&guest, &run_request(&["echo", "x"], |_| {})?).await?;
    let deleted = guest.deleted_files();
    for suffix in [".out", ".err", ".status"] {
        assert!(
            deleted.iter().any(|path| path.ends_with(suffix)),
            "{suffix}: {deleted:?}"
        );
    }
    for path in deleted {
        assert!(!std::path::Path::new(&path).exists(), "{path}");
    }
    Ok(())
}

#[tokio::test]
async fn a_stream_that_cannot_be_read_denies_the_command() -> TestResult {
    let guest = Guest::start().await?;
    guest.fail_downloads(".err", 500, 1).await;
    let response = within(
        guest
            .exec()?
            .handle_run_command(&guest.sandbox(), &run_request(&["echo", "x"], |_| {})?),
    )
    .await?;
    assert!(matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn a_receipt_that_cannot_be_read_denies_the_command() -> TestResult {
    let guest = Guest::start().await?;
    guest.fail_downloads(".status", 500, 1).await;
    let response = within(
        guest
            .exec()?
            .handle_run_command(&guest.sandbox(), &run_request(&["echo", "x"], |_| {})?),
    )
    .await?;
    assert!(matches!(response, RunCommandResponse::Denied(_)), "{response:?}");
    Ok(())
}

#[tokio::test]
async fn an_exit_code_of_124_is_not_mistaken_for_a_timeout() -> TestResult {
    let guest = Guest::start().await?;
    let output = run(&guest, &run_request(&["sh", "-c", "exit 124"], |_| {})?).await?;
    assert_eq!(output.exit_code, 124);
    assert_eq!(output.timed_out, CommandTimeoutStatus::WithinDeadline);
    Ok(())
}

#[tokio::test]
async fn a_command_that_ignores_term_is_still_ended_by_the_deadline() -> TestResult {
    let guest = Guest::start().await?;
    let request = run_request(&["sh", "-c", "trap '' TERM; sleep 30"], |wire| {
        wire["timeout_seconds"] = json!(1);
    })?;
    let started = Instant::now();
    let output = run(&guest, &request).await?;
    assert_eq!(output.timed_out, CommandTimeoutStatus::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(25));
    Ok(())
}
