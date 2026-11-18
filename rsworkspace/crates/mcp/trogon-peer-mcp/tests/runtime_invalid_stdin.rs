#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use trogon_peer_nats::test_support::broker::{Broker, TestResult};
use trogon_std::env::{ReadEnv as _, SystemEnv};

#[tokio::test]
async fn executable_write_only_stdin_reports_startup_failure_without_waiting_for_readiness() -> TestResult {
    let broker = Broker::start().await?;
    let (client, _) = broker.connect("alice").await?;
    client.flush().await?;
    let (_reader, writer) = std::io::pipe()?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-peer-mcp"));
    command
        .env_clear()
        .kill_on_drop(true)
        .env("NATS_URL", broker.server.address())
        .env("NATS_CA", broker.server.ca())
        .env(
            "NATS_NKEY",
            format!("file://{}", broker.credential_path("alice")?.display()),
        )
        .env("PEER_TENANT_ID", "tenant")
        .env("PEER_ACCOUNT_MODE", "shared_fixture")
        .env("PEER_CALLER_ID", "alice")
        .env("PEER_CONNECTION_ID", "one")
        .env("PEER_INSTALLATION_ID", "install")
        .env("PEER_COLLABORATION_REF", "shared")
        .env("PEER_DISPLAY_NAME", "alice")
        .env("PEER_AVAILABILITY", "ready")
        .stdin(Stdio::from(writer))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(profile) = SystemEnv.var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = tokio::time::timeout(Duration::from_secs(3), command.output()).await??;
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"Peer connector could not start.\n");
    Ok(())
}
