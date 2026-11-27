mod support;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use russh::client;
use support::{API_KEY, TestResult, start_gateway};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use trogon_sandbox_contract::TerminalDimension;
use trogon_sandbox_ssh_gateway::{GuestChannel, GuestError, PinnedHostKey, client_config, start_pty_channel};

type Pinned = Arc<Mutex<Option<String>>>;

async fn connect_pinned(addr: SocketAddr, pinned: &Pinned) -> Result<client::Handle<PinnedHostKey>, russh::Error> {
    client::connect(client_config(), addr, PinnedHostKey::new(Arc::clone(pinned))).await
}

async fn open(addr: SocketAddr, command: Option<&str>) -> TestResult<(client::Handle<PinnedHostKey>, GuestChannel)> {
    let pinned = Pinned::default();
    let mut session = connect_pinned(addr, &pinned).await?;
    let auth = session.authenticate_password("lease-1@gw", API_KEY).await?;
    assert!(auth.success());
    let channel = session.channel_open_session().await?;
    let size = (TerminalDimension::new(80)?, TerminalDimension::new(24)?);
    let guest = start_pty_channel(channel, size, "xterm", command.map(str::to_string)).await?;
    Ok((session, guest))
}

#[test]
fn client_config_keeps_the_connection_alive() {
    let config = client_config();
    assert!(config.keepalive_interval.is_some());
    assert_eq!(config.keepalive_max, 3);
}

#[tokio::test]
async fn the_first_host_key_is_pinned_and_a_different_one_refused() -> TestResult {
    let first = start_gateway(4).await?;
    let second = start_gateway(4).await?;
    let pinned = Pinned::default();
    connect_pinned(first.addr, &pinned).await?;
    assert!(pinned.lock().map_err(|error| error.to_string())?.is_some());
    connect_pinned(first.addr, &pinned).await?;
    assert!(connect_pinned(second.addr, &pinned).await.is_err());
    Ok(())
}

#[tokio::test]
async fn a_command_runs_and_reports_its_exit_status() -> TestResult {
    let gateway = start_gateway(4).await?;
    let (_session, mut guest) = open(gateway.addr, Some("echo hi")).await?;
    let mut output = String::new();
    guest.stdout.read_to_string(&mut output).await?;
    assert_eq!(output, "ran: echo hi");
    assert_eq!(guest.completion.await?, 3);
    Ok(())
}

#[tokio::test]
async fn a_shell_echoes_input_and_follows_resizes() -> TestResult {
    let gateway = start_gateway(4).await?;
    let (_session, mut guest) = open(gateway.addr, None).await?;
    guest
        .resize
        .send((TerminalDimension::new(100)?, TerminalDimension::new(30)?))
        .await?;
    guest.stdin.write_all(b"ping").await?;
    let mut seen = Vec::new();
    let mut buf = [0u8; 64];
    let mut read = 1;
    while read > 0 && !seen.ends_with(b"ping") {
        read = guest.stdout.read(&mut buf).await?;
        seen.extend_from_slice(&buf[..read]);
    }
    assert!(seen.ends_with(b"ping"), "the stream ended before the echo");
    guest.close.cancel();
    let result = guest.completion.await;
    assert!(matches!(result, Err(GuestError::NoExitStatus)));
    Ok(())
}

#[tokio::test]
async fn closing_the_input_ends_the_shell() -> TestResult {
    let gateway = start_gateway(4).await?;
    let (_session, mut guest) = open(gateway.addr, None).await?;
    drop(guest.stdin);
    let mut output = Vec::new();
    guest.stdout.read_to_end(&mut output).await?;
    assert_eq!(guest.completion.await?, 0);
    Ok(())
}

#[tokio::test]
async fn an_abandoned_output_stops_the_pump() -> TestResult {
    let gateway = start_gateway(4).await?;
    let (_session, guest) = open(gateway.addr, Some("echo hi")).await?;
    drop(guest.stdout);
    let result = guest.completion.await;
    assert!(matches!(result, Err(GuestError::NoExitStatus)));
    Ok(())
}

#[tokio::test]
async fn a_dropped_resize_sender_is_tolerated() -> TestResult {
    let gateway = start_gateway(4).await?;
    let (_session, mut guest) = open(gateway.addr, Some("echo hi")).await?;
    drop(guest.resize);
    let mut output = String::new();
    guest.stdout.read_to_string(&mut output).await?;
    assert_eq!(guest.completion.await?, 3);
    Ok(())
}

#[tokio::test]
async fn a_closed_channel_fails_the_request() -> TestResult {
    let gateway = start_gateway(4).await?;
    let pinned = Pinned::default();
    let mut session = connect_pinned(gateway.addr, &pinned).await?;
    session.authenticate_password("lease-1@gw", API_KEY).await?;
    let channel = session.channel_open_session().await?;
    session
        .disconnect(russh::Disconnect::ByApplication, "bye", "en")
        .await?;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let size = (TerminalDimension::new(80)?, TerminalDimension::new(24)?);
    let result = start_pty_channel(channel, size, "xterm", None).await;
    assert!(matches!(result, Err(GuestError::Ssh(_))));
    Ok(())
}
