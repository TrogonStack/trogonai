mod command_support;
mod common;

use std::time::Duration;

use command_support::{execution_id, is_not_running, not_running};
use common::{TestResult, client_replying, lease_id};
use trogon_sandbox_contract::{
    CommandExitReport, LogChunk, LogsContinuation, OutputStream, OutputTruncation, ReadLogsResponse,
};

#[tokio::test]
async fn read_logs_returns_unsupported_without_store() -> TestResult {
    let (client, log) = client_replying(&ReadLogsResponse::Unsupported)?;
    let result = client.read_logs(&lease_id(), &execution_id(), 0, None).await?;
    assert_eq!(result, ReadLogsResponse::Unsupported);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.lease.read_logs.lease-1");
    assert_eq!(sent.timeout, Duration::from_secs(10));
    Ok(())
}

#[tokio::test]
async fn read_logs_waiting_sends_the_cursor_and_extends_the_deadline() -> TestResult {
    let response = ReadLogsResponse::Logs {
        chunks: vec![LogChunk {
            log_seq: 4,
            stream: OutputStream::Stdout,
            seq: 0,
            data: b"x".to_vec(),
        }],
        next_seq: 4,
        has_more: LogsContinuation::Complete,
        truncated: OutputTruncation::Complete,
        exited: CommandExitReport::NotReported,
    };
    let (client, log) = client_replying(&response)?;
    let result = client
        .read_logs_waiting(
            &lease_id(),
            &execution_id(),
            3,
            Some(1024),
            Some(Duration::from_millis(1500)),
        )
        .await?;
    assert_eq!(result, response);
    let sent = log.only()?;
    assert_eq!(sent.timeout, Duration::from_millis(11_500));
    let body = sent.json()?;
    assert_eq!(body["after_seq"], 3);
    assert_eq!(body["max_bytes"], 1024);
    assert_eq!(body["wait_ms"], 1500);
    Ok(())
}

#[tokio::test]
async fn read_logs_returns_denied_when_broker_denies() -> TestResult {
    let (client, _) = client_replying(&ReadLogsResponse::Denied(not_running()))?;
    assert!(is_not_running(
        &client.read_logs(&lease_id(), &execution_id(), 0, None).await
    ));
    Ok(())
}
