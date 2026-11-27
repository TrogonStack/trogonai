//! Interactive stdin of detached commands against a guest that runs them on this machine.

mod guest_support;

use guest_support::*;
use serde_json::json;
use trogon_sandbox_contract::{ExecutionId, StdinChunk, StdinEof, StdinSeq, WriteStdinRequest, WriteStdinResponse};

fn interactive(wire: &mut serde_json::Value) {
    wire["detached"] = json!(true);
    wire["stdin"] = json!("interactive");
}

fn write(execution: &str, seq: u64, data: &[u8], eof: bool) -> Result<WriteStdinRequest, Boxed> {
    Ok(WriteStdinRequest::new(
        lease_id(),
        ExecutionId::new(execution),
        StdinSeq::new(seq),
        StdinChunk::new(data.to_vec())?,
        StdinEof::from(eof),
    )?)
}

fn next(response: &WriteStdinResponse) -> Option<u64> {
    match response {
        WriteStdinResponse::Written { next_seq }
        | WriteStdinResponse::Duplicate { next_seq }
        | WriteStdinResponse::Buffered { next_seq }
        | WriteStdinResponse::OutOfWindow { next_seq } => Some(next_seq.get()),
        _ => None,
    }
}

#[tokio::test]
async fn written_bytes_reach_the_command_until_stdin_closes() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["cat"], interactive)?).await?;
    let first = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"hello ", false)?)
        .await;
    assert_eq!(next(&first), Some(1), "{first:?}");
    let second = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 1, b"world\n", true)?)
        .await;
    assert_eq!(next(&second), Some(2), "{second:?}");
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.stdout, b"hello world\n");
    assert_eq!(output.exit_code, 0);
    Ok(())
}

#[tokio::test]
async fn binary_stdin_round_trips() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["cat"], interactive)?).await?;
    let bytes: Vec<u8> = vec![0, 1, 255, 10, 13, 0];
    exec.handle_write_stdin(&guest.sandbox(), &write(&id, 0, &bytes, true)?)
        .await;
    assert_eq!(within(finish(&guest, &exec, &id)).await??.stdout, bytes);
    Ok(())
}

#[tokio::test]
async fn a_resent_write_is_applied_once() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["cat"], interactive)?).await?;
    exec.handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"a", false)?)
        .await;
    let again = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"a", false)?)
        .await;
    assert!(matches!(again, WriteStdinResponse::Duplicate { .. }), "{again:?}");
    exec.handle_write_stdin(&guest.sandbox(), &write(&id, 1, b"b", true)?)
        .await;
    assert_eq!(within(finish(&guest, &exec, &id)).await??.stdout, b"ab");
    Ok(())
}

#[tokio::test]
async fn a_write_ahead_of_order_waits_for_the_gap() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["cat"], interactive)?).await?;
    let early = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 1, b"b", true)?)
        .await;
    assert!(matches!(early, WriteStdinResponse::Buffered { .. }), "{early:?}");
    exec.handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"a", false)?)
        .await;
    assert_eq!(within(finish(&guest, &exec, &id)).await??.stdout, b"ab");
    Ok(())
}

#[tokio::test]
async fn a_closed_stdin_refuses_more_writes() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["sleep", "2"], interactive)?).await?;
    exec.handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"x", true)?)
        .await;
    let after = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 1, b"y", false)?)
        .await;
    assert!(matches!(after, WriteStdinResponse::StdinClosed), "{after:?}");
    within(finish(&guest, &exec, &id)).await??;
    Ok(())
}

#[tokio::test]
async fn a_command_without_interactive_stdin_refuses_writes() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["sleep", "2"], |wire| wire["detached"] = json!(true))?;
    let id = start(&guest, &exec, &request).await?;
    let response = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"x", false)?)
        .await;
    assert!(
        matches!(response, WriteStdinResponse::StdinNotInteractive),
        "{response:?}"
    );
    within(finish(&guest, &exec, &id)).await??;
    Ok(())
}

#[tokio::test]
async fn an_unknown_or_finished_execution_is_reported() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let unknown = exec
        .handle_write_stdin(&guest.sandbox(), &write("nope", 0, b"x", false)?)
        .await;
    assert!(matches!(unknown, WriteStdinResponse::NotFound));
    let id = start(&guest, &exec, &run_request(&["true"], interactive)?).await?;
    within(finish(&guest, &exec, &id)).await??;
    let finished = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"x", false)?)
        .await;
    assert!(matches!(finished, WriteStdinResponse::AlreadyCompleted), "{finished:?}");
    Ok(())
}

#[tokio::test]
async fn an_input_call_that_fails_closes_stdin_for_good() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let id = start(&guest, &exec, &run_request(&["sleep", "2"], interactive)?).await?;
    guest.fail_inputs(Some(503)).await;
    let failed = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"x", false)?)
        .await;
    assert!(matches!(failed, WriteStdinResponse::StdinClosed), "{failed:?}");
    guest.fail_inputs(None).await;
    let later = exec
        .handle_write_stdin(&guest.sandbox(), &write(&id, 1, b"y", false)?)
        .await;
    assert!(matches!(later, WriteStdinResponse::StdinClosed), "{later:?}");
    within(finish(&guest, &exec, &id)).await??;
    Ok(())
}

#[tokio::test]
async fn a_finite_deadline_keeps_interactive_stdin_working() -> TestResult {
    let guest = Guest::start().await?;
    let exec = guest.exec()?;
    let request = run_request(&["cat"], |wire| {
        interactive(wire);
        wire["timeout_seconds"] = json!(30);
    })?;
    let id = start(&guest, &exec, &request).await?;
    exec.handle_write_stdin(&guest.sandbox(), &write(&id, 0, b"through\n", true)?)
        .await;
    let output = within(finish(&guest, &exec, &id)).await??;
    assert_eq!(output.stdout, b"through\n");
    assert!(
        guest
            .session_commands()
            .iter()
            .any(|command| command.contains("timeout -k"))
    );
    Ok(())
}
