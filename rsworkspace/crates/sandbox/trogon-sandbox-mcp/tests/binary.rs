use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"binary-test","version":"0"}}}"#;

#[tokio::test]
async fn the_binary_answers_the_handshake_and_exits_when_stdin_closes() -> TestResult {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-mcp"))
        .args([
            "--tenant",
            "acme",
            "--nats-url",
            "nats://127.0.0.1:1",
            "--log-level",
            "error",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let stdout = child.stdout.take().ok_or("no stdout")?;
    stdin.write_all(INITIALIZE.as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    let mut lines = BufReader::new(stdout).lines();
    let reply = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await??
        .ok_or("the server closed without answering")?;
    assert!(reply.contains("\"id\":1"));
    assert!(reply.contains("protocolVersion"));
    drop(stdin);
    let exit = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    assert!(exit.success());
    Ok(())
}
