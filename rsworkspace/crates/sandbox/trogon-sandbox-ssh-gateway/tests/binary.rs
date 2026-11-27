#![cfg(unix)]

mod support;

use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use russh::ChannelMsg;
use support::{FakeHost, TestResult, eventually_async, json, respond};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyBucket, NatsKvApiKeyStore};
use trogon_sandbox_contract::{
    CredentialAck, CredentialAckOutcome, CredentialIssueOutcome, CredentialIssueResponse, ServiceAccountId,
    ShortLivedToken, TenantId, TerminalOutput, TokenId,
};
use trogon_sandbox_protocol::subjects::terminal_output;
use trogon_sandbox_protocol::terminal_data_frame;
use trogon_sandbox_ssh_gateway::{BridgeLimits, SshStreamBridge};

const BINARY: &str = env!("CARGO_BIN_EXE_trogon-sandbox-ssh-gateway");
const ISSUE: &str = "sandbox.credentials.issue.ssh.platform";
const REVOKE: &str = "sandbox.credentials.revoke.ssh.platform";

async fn free_addr() -> TestResult<SocketAddr> {
    let probe = TcpListener::bind("127.0.0.1:0").await?;
    let addr = probe.local_addr()?;
    drop(probe);
    Ok(addr)
}

fn gateway(nats_url: &str, listen: SocketAddr, host_key: &Path) -> Command {
    let mut command = Command::new(BINARY);
    command
        .args(["--nats-url", nats_url])
        .args(["--listen", &listen.to_string()])
        .args(["--metrics-addr", "127.0.0.1:0"])
        .arg("--host-key-path")
        .arg(host_key)
        .env_remove("SANDBOX_DISPATCH_SIGNING_KEY")
        .env_remove("SANDBOX_DISPATCH_SIGNING_KEY_FILE")
        .env_remove("SANDBOX_DISPATCH_VERIFY")
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
    let dir = tempfile::tempdir()?;
    let status = failing_start(&mut gateway(
        "nats://127.0.0.1:1",
        free_addr().await?,
        &dir.path().join("key"),
    ))
    .await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn a_server_without_jetstream_stops_the_gateway() -> TestResult {
    let server = support::Server::start().await;
    let dir = tempfile::tempdir()?;
    let status = failing_start(&mut gateway(&server.url(), free_addr().await?, &dir.path().join("key"))).await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn an_unusable_host_key_path_stops_the_gateway() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let dir = tempfile::tempdir()?;
    let status = failing_start(&mut gateway(
        &format!("nats://{}", server.address()),
        free_addr().await?,
        dir.path(),
    ))
    .await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn an_invalid_signing_key_stops_the_gateway() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let dir = tempfile::tempdir()?;
    let status = failing_start(
        gateway(
            &format!("nats://{}", server.address()),
            free_addr().await?,
            &dir.path().join("key"),
        )
        .env("SANDBOX_DISPATCH_SIGNING_KEY", "not base64"),
    )
    .await?;
    assert!(!status.success());
    Ok(())
}

#[tokio::test]
async fn an_occupied_port_stops_the_gateway_and_an_absent_broker_is_tolerated() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let dir = tempfile::tempdir()?;
    let occupied = TcpListener::bind("127.0.0.1:0").await?;
    let status = failing_start(
        gateway(
            &format!("nats://{}", server.address()),
            occupied.local_addr()?,
            &dir.path().join("key"),
        )
        .args(["--credential-broker-url", "nats://127.0.0.1:1"]),
    )
    .await?;
    assert!(!status.success());
    Ok(())
}

async fn serves_a_session(answer_revoke: bool) -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let client = server.client().await;
    let store = NatsKvApiKeyStore::new_with_bucket(server.jetstream().await, &ApiKeyBucket::default()).await?;
    let key = store
        .mint(TenantId::new("tenant-a")?, ServiceAccountId::new("sa-1"), "binary")
        .await?;

    let issued = json(&CredentialIssueOutcome::Ok(CredentialIssueResponse {
        token_id: TokenId::new("tok-1"),
        token: ShortLivedToken::new("secret"),
        expires_at: UNIX_EPOCH + Duration::from_secs(100),
    }))?;
    respond(&client, ISSUE, move |_| Some(issued.clone())).await?;
    if answer_revoke {
        let ack = json(&CredentialAckOutcome::Ok(CredentialAck { message: None }))?;
        respond(&client, REVOKE, move |_| Some(ack.clone())).await?;
    }

    let host = FakeHost::holding(&["lease-1"]);
    let bridge = SshStreamBridge::new(host.clone(), client.clone(), BridgeLimits::default());
    let cancel = CancellationToken::new();
    let worker = tokio::spawn(Arc::clone(&bridge).serve(client.clone(), cancel.clone()));

    let dir = tempfile::tempdir()?;
    let listen = free_addr().await?;
    let seed = STANDARD.encode([7u8; 32]);
    let child = gateway(&url, listen, &dir.path().join("key"))
        .env("SANDBOX_DISPATCH_SIGNING_KEY", seed)
        .spawn()?;

    let mut session = None;
    for _ in 0..100 {
        if let Ok(connected) = support::connect(listen).await {
            session = Some(connected);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut session = session.ok_or("the gateway did not listen")?;
    let auth = session.authenticate_password("lease-1@gw", key.expose_secret()).await?;
    assert!(auth.success());
    let mut channel = session.channel_open_session().await?;
    channel.exec(true, "echo hi").await?;

    eventually_async(|| async { host.first_terminal().is_some() }).await?;
    let terminal = host.first_terminal().ok_or("no terminal")?;
    client
        .publish(terminal_output(&terminal), json(&terminal_data_frame(0, b"hello")?)?)
        .await?;
    client
        .publish(
            terminal_output(&terminal),
            json(&TerminalOutput::Exit {
                seq: 1,
                exit_code: Some(0),
            })?,
        )
        .await?;
    client.flush().await?;

    let mut output = Vec::new();
    while let Some(message) = tokio::time::timeout(Duration::from_secs(10), channel.wait()).await? {
        if let ChannelMsg::Data { data } = message {
            output.extend_from_slice(&data);
        }
    }
    assert_eq!(output, b"hello");
    drop(channel);
    drop(session);
    tokio::time::sleep(Duration::from_millis(500)).await;

    let signalled = interrupt(&child)?;
    let status = tokio::task::spawn_blocking(move || wait_for_exit(child).map_err(|error| error.to_string())).await??;
    cancel.cancel();
    worker.await??;
    assert!(signalled);
    assert!(status.success());
    Ok(())
}

#[tokio::test]
async fn the_binary_serves_a_session_and_exits_on_interrupt() -> TestResult {
    serves_a_session(true).await
}

#[tokio::test]
async fn a_failed_credential_revoke_does_not_stop_the_session() -> TestResult {
    serves_a_session(false).await
}
