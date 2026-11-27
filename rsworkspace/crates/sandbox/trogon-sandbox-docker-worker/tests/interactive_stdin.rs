//! Interactive-stdin (bytes written to a detached exec's stdin) integration tests.
//!
//! ```text
//! cargo test -p trogon-sandbox-docker-worker interactive_stdin
//! ```

mod docker_support;

use docker_support::{TestResult, build_provider, fresh_lease_id, lease_request, wait_completed};

use trogon_sandbox_contract::{
    BootMode, CommandDeadline, CommandEnv, ExecutionId, LeaseId, LeaseRequest, OutputRetention, StdinChunk, StdinEof,
    StdinMode, StdinSeq, StreamedOutput, WriteStdinResponse,
};
use trogon_sandbox_docker_worker::adapter::DockerWorkerProvider;
use trogon_sandbox_protocol::Provider;

fn chunk(bytes: &[u8]) -> TestResult<StdinChunk> {
    Ok(StdinChunk::new(bytes.to_vec())?)
}

async fn start_cat(provider: &DockerWorkerProvider, lease_id: &LeaseId, stdin: StdinMode) -> TestResult<ExecutionId> {
    Ok(provider
        .run_command_for_lease_detached(
            lease_id,
            vec!["cat".to_string()],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            stdin,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?)
}

async fn lease(provider: &DockerWorkerProvider) -> TestResult<LeaseId> {
    let lease_id = fresh_lease_id();
    provider.lease(&bare_lease_request()?, &lease_id).await?;
    Ok(lease_id)
}

fn bare_lease_request() -> TestResult<LeaseRequest> {
    lease_request("interactive-stdin-test", BootMode::Bare)
}

#[tokio::test(flavor = "multi_thread")]
async fn write_stdin_reaches_process_and_eof_completes_it() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = start_cat(&provider, &lease_id, StdinMode::Interactive).await?;

    let first = provider
        .write_stdin_for_lease(
            &lease_id,
            &execution_id,
            StdinSeq::new(0),
            chunk(b"hello ")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(
        first,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(1)
        }
    );
    let second = provider
        .write_stdin_for_lease(
            &lease_id,
            &execution_id,
            StdinSeq::new(1),
            chunk(b"world")?,
            StdinEof::Close,
        )
        .await;
    assert_eq!(
        second,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(2)
        }
    );

    let output = wait_completed(&provider, &lease_id, &execution_id, 10).await?;
    assert_eq!(output.exit_code, 0);
    assert_eq!(output.stdout, b"hello world");

    let late = provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(2), chunk(b"x")?, StdinEof::Keep)
        .await;
    assert_eq!(late, WriteStdinResponse::AlreadyCompleted);

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_write_is_not_applied_twice() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = start_cat(&provider, &lease_id, StdinMode::Interactive).await?;

    let written = provider
        .write_stdin_for_lease(
            &lease_id,
            &execution_id,
            StdinSeq::new(0),
            chunk(b"once")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(
        written,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(1)
        }
    );
    let duplicate = provider
        .write_stdin_for_lease(
            &lease_id,
            &execution_id,
            StdinSeq::new(0),
            chunk(b"once")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(
        duplicate,
        WriteStdinResponse::Duplicate {
            next_seq: StdinSeq::new(1)
        }
    );
    provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(1), chunk(b"")?, StdinEof::Close)
        .await;

    let output = wait_completed(&provider, &lease_id, &execution_id, 10).await?;
    assert_eq!(output.stdout, b"once");

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn out_of_order_writes_are_applied_in_order() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = start_cat(&provider, &lease_id, StdinMode::Interactive).await?;

    let ahead = provider
        .write_stdin_for_lease(
            &lease_id,
            &execution_id,
            StdinSeq::new(1),
            chunk(b"two")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(
        ahead,
        WriteStdinResponse::Buffered {
            next_seq: StdinSeq::new(0)
        }
    );
    let filled = provider
        .write_stdin_for_lease(
            &lease_id,
            &execution_id,
            StdinSeq::new(0),
            chunk(b"one-")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(
        filled,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(2)
        }
    );
    provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(2), chunk(b"")?, StdinEof::Close)
        .await;

    let output = wait_completed(&provider, &lease_id, &execution_id, 10).await?;
    assert_eq!(output.stdout, b"one-two");

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_stdin_after_eof_reports_stdin_closed() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec!["sleep".to_string(), "30".to_string()],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Interactive,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let closed = provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(0), chunk(b"")?, StdinEof::Close)
        .await;
    assert_eq!(
        closed,
        WriteStdinResponse::Written {
            next_seq: StdinSeq::new(1)
        }
    );
    let after = provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(1), chunk(b"x")?, StdinEof::Keep)
        .await;
    assert_eq!(after, WriteStdinResponse::StdinClosed);

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_stdin_with_wrong_lease_reports_not_found() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = start_cat(&provider, &lease_id, StdinMode::Interactive).await?;

    let other_lease = LeaseId::new("some-other-lease");
    let response = provider
        .write_stdin_for_lease(
            &other_lease,
            &execution_id,
            StdinSeq::new(0),
            chunk(b"x")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(response, WriteStdinResponse::NotFound);
    assert!(
        provider
            .poll_command_for_lease(&other_lease, &execution_id)
            .await
            .is_none()
    );

    provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(0), chunk(b"")?, StdinEof::Close)
        .await;
    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_stdin_to_non_interactive_command_reports_not_interactive() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec!["sleep".to_string(), "30".to_string()],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::Buffer,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let response = provider
        .write_stdin_for_lease(&lease_id, &execution_id, StdinSeq::new(0), chunk(b"x")?, StdinEof::Keep)
        .await;
    assert_eq!(response, WriteStdinResponse::StdinNotInteractive);

    provider.cancel(&lease_id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn write_stdin_to_unknown_execution_reports_not_found() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;

    let lease_id = LeaseId::new("no-such-lease");
    let response = provider
        .write_stdin_for_lease(
            &lease_id,
            &ExecutionId::new("no-such-execution-id"),
            StdinSeq::new(0),
            chunk(b"x")?,
            StdinEof::Keep,
        )
        .await;
    assert_eq!(response, WriteStdinResponse::NotFound);
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn stream_only_retention_keeps_no_stdout() -> TestResult {
    let provider = build_provider("interactive_stdin").await?;
    let lease_id = lease(&provider).await?;
    let execution_id = provider
        .run_command_for_lease_detached(
            &lease_id,
            vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo out; echo err 1>&2".to_string(),
            ],
            None,
            &CommandEnv::default(),
            60,
            None,
            StreamedOutput::Disabled,
            StdinMode::Closed,
            OutputRetention::StreamOnly,
            CommandDeadline::Timeout,
            usize::MAX,
            None,
        )
        .await?;

    let output = wait_completed(&provider, &lease_id, &execution_id, 10).await?;
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"err\n");

    provider.cancel(&lease_id).await?;
    Ok(())
}
