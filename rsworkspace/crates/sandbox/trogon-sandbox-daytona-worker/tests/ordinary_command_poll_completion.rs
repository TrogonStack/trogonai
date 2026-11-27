//! Polling a detached command that runs under the pinned supervisor. The lanes that start a real supervisor need a
//! Linux host and the two `SANDBOX_DAYTONA_COMMAND_SUPERVISOR_*` variables; without them they return at once.

mod guest_support;

use guest_support::*;
use serde_json::json;
use trogon_sandbox_contract::PollCommandResponse;

fn detached(wire: &mut serde_json::Value) {
    wire["detached"] = json!(true);
}

#[tokio::test]
async fn a_failed_state_read_after_the_end_does_not_lose_the_real_completion() -> TestResult {
    let guest = Guest::start().await?;
    let Some(exec) = guest.exec_with_supervisor()? else {
        return Ok(());
    };
    let request = run_request(&["/bin/sh", "-c", "echo out; echo err >&2; exit 5"], detached)?;
    let id = start(&guest, &exec, &request).await?;
    assert!(guest.settle().await, "the bridge never ended");
    guest.fail_downloads(".state", 503, 1).await;
    let first = exec.handle_poll_command(&guest.sandbox(), LEASE, &id).await;
    assert!(matches!(first, PollCommandResponse::Denied(_)), "{first:?}");
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.exit_code, 5);
    assert_eq!(output.stdout, b"out\n");
    assert_eq!(output.stderr, b"err\n");
    Ok(())
}

#[tokio::test]
async fn the_result_is_kept_after_the_sandbox_files_are_removed() -> TestResult {
    let guest = Guest::start().await?;
    let Some(exec) = guest.exec_with_supervisor()? else {
        return Ok(());
    };
    let id = start(&guest, &exec, &run_request(&["/bin/sh", "-c", "echo kept"], detached)?).await?;
    let first = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(first.stdout, b"kept\n");
    let (_, left) = guest
        .shell(&format!("ls /tmp/.trogon-exec-{id}.* 2>/dev/null | wc -l"))
        .await?;
    assert_eq!(left.trim(), "0");
    let second = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(second.stdout, b"kept\n");
    assert!(guest.session_deletes() >= 1);
    Ok(())
}

#[tokio::test]
async fn polling_an_unknown_execution_is_not_found() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let response = exec.handle_poll_command(&guest.sandbox(), LEASE, "missing").await;
    assert!(matches!(response, PollCommandResponse::NotFound), "{response:?}");
    Ok(())
}
