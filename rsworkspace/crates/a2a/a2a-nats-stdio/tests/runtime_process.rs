use std::io;
use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use trogon_nats::test_support::CoreTestServer;
use trogon_std::env::{EnumerateEnv, SystemEnv};

#[derive(Debug, thiserror::Error)]
enum FixtureError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Timeout(#[from] tokio::time::error::Elapsed),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("child stdin pipe was not created")]
    MissingStdin,
}

enum ParentOutput {
    Reading,
    Closed,
}

async fn run_process(settings: &[(&str, &str)], input: &[u8], output: ParentOutput) -> Result<Output, FixtureError> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-nats-stdio"));
    command
        .env_clear()
        .envs(SystemEnv.vars_os().into_iter().filter(|(key, _)| {
            matches!(
                key.to_str(),
                Some("LLVM_PROFILE_FILE" | "LD_LIBRARY_PATH" | "DYLD_FALLBACK_LIBRARY_PATH")
            )
        }))
        .env("A2A_CONNECT_TIMEOUT_SECS", "1")
        .envs(settings.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    if matches!(output, ParentOutput::Closed) {
        drop(child.stdout.take());
    }
    let mut stdin = child.stdin.take().ok_or(FixtureError::MissingStdin)?;
    stdin.write_all(input).await?;
    drop(stdin);
    Ok(tokio::time::timeout(Duration::from_secs(15), child.wait_with_output()).await??)
}

#[tokio::test]
async fn invalid_startup_configuration_exits_with_a_diagnostic() -> Result<(), FixtureError> {
    for (settings, expected) in [
        (vec![], "A2A_AGENT_ID environment variable is required"),
        (vec![("A2A_PREFIX", "bad prefix")], "invalid A2A prefix"),
        (vec![("A2A_AGENT_ID", "bad agent")], "invalid agent id"),
        (
            vec![("A2A_AGENT_ID", "worker"), ("NATS_URL", "invalid://server")],
            "NATS connection failed",
        ),
    ] {
        let output = run_process(&settings, b"", ParentOutput::Reading).await?;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
    }
    Ok(())
}

#[tokio::test]
async fn connected_runtime_reports_parse_errors_and_drains_output_on_eof() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let output = run_process(
        &[
            ("A2A_AGENT_ID", "worker"),
            ("A2A_PREFIX", "tenant"),
            ("NATS_URL", server.address()),
            ("A2A_OPERATION_TIMEOUT_SECS", "1"),
        ],
        b"invalid json\n",
        ParentOutput::Reading,
    )
    .await?;
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let frame: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(frame["jsonrpc"], "2.0");
    assert_eq!(frame["id"], serde_json::Value::Null);
    assert_eq!(frame["error"]["code"], -32700);
    assert!(output.stdout.ends_with(b"\n"));
    Ok(())
}

#[tokio::test]
async fn closed_parent_output_causes_a_nonzero_runtime_exit() -> Result<(), FixtureError> {
    let server = CoreTestServer::start().await;
    let output = run_process(
        &[("A2A_AGENT_ID", "worker"), ("NATS_URL", server.address())],
        b"invalid json\n",
        ParentOutput::Closed,
    )
    .await?;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("stdio loop failed"));
    Ok(())
}
