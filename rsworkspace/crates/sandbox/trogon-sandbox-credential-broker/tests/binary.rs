use std::error::Error;
use std::process::Stdio;
use std::time::Duration;

use trogon_nats::test_support::JetStreamTestServer;

const KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

#[tokio::test]
async fn the_binary_serves_until_interrupted() -> Result<(), Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-credential-broker"))
        .args(["--nats-url", server.address(), "--kms-key", KEY])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let pid = child.id().ok_or("the broker exited before it was interrupted")?;
    let client = server.client().await;
    // A malformed request is never answered once the broker is subscribed; before that it has no responders.
    let mut subscribed = false;
    for _ in 0..100 {
        let probe = tokio::time::timeout(
            Duration::from_millis(300),
            client.request("sandbox.credentials.revoke.registry", bytes::Bytes::from_static(b"{")),
        )
        .await;
        if probe.is_err() {
            subscribed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(subscribed);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status = tokio::process::Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .await?;
    assert!(status.success());
    let exit = tokio::time::timeout(Duration::from_secs(20), child.wait()).await??;
    assert!(exit.success());
    Ok(())
}

#[tokio::test]
async fn the_binary_refuses_a_missing_key() -> Result<(), Box<dyn Error>> {
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-credential-broker"))
        .env_remove("SANDBOX_KMS_KEY")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    assert!(!status.success());
    Ok(())
}
