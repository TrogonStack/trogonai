mod nats_common;

use std::process::Stdio;
use std::time::Duration;

use bytes::Bytes;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn the_binary_serves_and_exits_cleanly_on_interrupt() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = server.client().await?;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-memory-worker"))
        .env("SANDBOX_NATS_URL", server.url())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut served = false;
    for _ in 0..200 {
        let reply = tokio::time::timeout(
            Duration::from_millis(200),
            client.request("sandbox.provider.mock.lease", Bytes::from_static(b"{}")),
        )
        .await;
        if matches!(reply, Ok(Ok(_))) {
            served = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(served, "the worker never answered");
    let pid = child.id().ok_or("the worker exited early")?;
    let status = tokio::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .await?;
    assert!(status.success());
    let exit = tokio::time::timeout(Duration::from_secs(20), child.wait()).await??;
    assert!(exit.success());
    Ok(())
}
