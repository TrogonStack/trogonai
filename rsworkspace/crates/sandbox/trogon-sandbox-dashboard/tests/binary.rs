#![cfg(unix)]

use std::process::{Command, Stdio};
use std::time::Duration;

use tokio::net::TcpListener;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[tokio::test]
async fn binary_serves_and_exits_on_interrupt() -> TestResult {
    let probe = TcpListener::bind("127.0.0.1:0").await?;
    let listen = probe.local_addr()?;
    drop(probe);
    let mut child = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-dashboard"))
        .args(["--listen", &listen.to_string(), "--metrics-addr", "127.0.0.1:0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let mut served = false;
    for _ in 0..100 {
        if let Ok(response) = reqwest::get(format!("http://{listen}/config.json")).await {
            served = response.status().is_success();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let signalled = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()?
        .success();
    let status = child.wait()?;

    assert!(served);
    assert!(signalled);
    assert!(status.success());
    Ok(())
}
