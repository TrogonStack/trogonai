use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn executable_reports_credential_startup_failure_without_shutdown_uncertainty() -> TestResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-peer-mcp"));
    command
        .env_clear()
        .kill_on_drop(true)
        .env("NATS_NKEY", "env://UNSET_PEER_TEST_NKEY")
        .env("PEER_TENANT_ID", "tenant")
        .env("PEER_ACCOUNT_MODE", "shared_fixture")
        .env("PEER_CALLER_ID", "alice")
        .env("PEER_CONNECTION_ID", "one")
        .env("PEER_INSTALLATION_ID", "install")
        .env("PEER_COLLABORATION_REF", "shared")
        .env("PEER_DISPLAY_NAME", "alice")
        .env("PEER_AVAILABILITY", "ready")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let output = tokio::time::timeout(Duration::from_secs(3), command.output()).await??;
    assert!(!output.status.success(), "invalid credential reference was admitted");
    let diagnostic = String::from_utf8(output.stderr)?;
    assert!(
        diagnostic.contains("Peer connector could not start."),
        "startup phase was not reported: {diagnostic}"
    );
    assert!(
        !diagnostic.contains("remote outcomes remain uncertain"),
        "startup was reported as uncertain shutdown"
    );
    assert!(output.stdout.is_empty(), "startup failure wrote MCP protocol output");
    Ok(())
}
