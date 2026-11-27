#![cfg(unix)]

mod support;

use std::net::SocketAddr;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

use tokio::net::TcpListener;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyBucket, NatsKvApiKeyStore};
use trogon_sandbox_contract::{ServiceAccountId, TenantId};
use trogon_sandbox_ws_gateway::API_KEY_HEADER;

use support::TestResult;

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-ws-gateway");

async fn free_addr() -> TestResult<SocketAddr> {
    let probe = TcpListener::bind("127.0.0.1:0").await?;
    let addr = probe.local_addr()?;
    drop(probe);
    Ok(addr)
}

fn gateway(nats_url: &str, listen: SocketAddr) -> Command {
    let mut command = Command::new(BINARY);
    command
        .args(["--nats-url", nats_url])
        .args(["--listen", &listen.to_string()])
        .args(["--metrics-addr", "127.0.0.1:0"])
        .env_remove("SANDBOX_PREVIEW_SIGNING_KEYS")
        .env_remove("SANDBOX_PREVIEW_ACTIVE_KEY_ID")
        .env_remove("SANDBOX_PREVIEW_PUBLIC_URL")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn wait_for_exit(mut child: Child) -> TestResult<ExitStatus> {
    let polled = (0..300).find_map(|_| {
        let polled = child.try_wait().transpose();
        if polled.is_none() {
            std::thread::sleep(Duration::from_millis(100));
        }
        polled
    });
    // A child that already exited is not an error to kill; one that did not must not outlive the test.
    let _ = child.kill();
    Ok(polled.ok_or("the gateway did not exit")??)
}

fn interrupt(child: &Child) -> TestResult<bool> {
    Ok(Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()?
        .success())
}

async fn failing_start(command: &mut Command) -> TestResult<ExitStatus> {
    let child = command.spawn()?;
    tokio::task::spawn_blocking(move || wait_for_exit(child).map_err(|error| error.to_string()))
        .await?
        .map_err(Into::into)
}

#[tokio::test]
async fn an_unreachable_nats_server_stops_the_gateway() -> TestResult {
    let status = failing_start(&mut gateway("nats://127.0.0.1:1", free_addr().await?)).await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn an_occupied_port_stops_the_gateway() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let occupied = TcpListener::bind("127.0.0.1:0").await?;
    let status = failing_start(&mut gateway(
        &format!("nats://{}", server.address()),
        occupied.local_addr()?,
    ))
    .await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn the_binary_serves_a_session_and_exits_on_interrupt() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let store = NatsKvApiKeyStore::new_with_bucket(server.jetstream().await, &ApiKeyBucket::default()).await?;
    let key = store
        .mint(TenantId::new("tenant-a")?, ServiceAccountId::new("sa-1"), "binary")
        .await?;
    let listen = free_addr().await?;
    let child = gateway(&format!("nats://{}", server.address()), listen).spawn()?;

    let client = reqwest::Client::new();
    let mut status = None;
    for _ in 0..100 {
        let attempt = client
            .post(format!("http://{listen}/v1/sessions"))
            .header(API_KEY_HEADER, key.expose_secret())
            .send()
            .await;
        if let Ok(response) = attempt {
            status = Some(response.status());
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let signalled = interrupt(&child)?;
    let exit = tokio::task::spawn_blocking(move || wait_for_exit(child).map_err(|error| error.to_string())).await??;
    assert_eq!(status.ok_or("the gateway did not listen")?.as_u16(), 201);
    assert!(signalled);
    assert!(exit.success());
    Ok(())
}
