use std::time::Duration;

use async_nats::{Client, HeaderMap};
use bytes::Bytes;
use futures::StreamExt;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_client::{Bus, BusError, ClientError, NatsBus, SandboxClient};
use trogon_sandbox_contract::{LeaseId, TerminalId, TerminalOp, TerminalResponse};
use trogon_sandbox_protocol::NatsTransferError;
use trogon_sandbox_protocol::nats_transfer::{receive, respond};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const STEP: Duration = Duration::from_secs(10);

fn url(server: &CoreTestServer) -> String {
    format!("nats://{}", server.address())
}

async fn started() -> Result<(CoreTestServer, Client), Box<dyn std::error::Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(url(&server)).await?;
    Ok((server, client))
}

/// Answers every request on `subject` with `prefix` followed by the request body, and records the `Authorization`
/// header it saw in a second reply header.
async fn spawn_echo(client: Client, subject: &'static str) -> TestResult {
    let mut requests = client.subscribe(subject).await?;
    client.flush().await?;
    tokio::spawn(async move {
        while let Some(message) = requests.next().await {
            let Some(reply) = message.reply.clone() else {
                continue;
            };
            let auth = message
                .headers
                .as_ref()
                .and_then(|headers| headers.get("Authorization"))
                .map(|value| value.to_string())
                .unwrap_or_else(|| "none".to_string());
            let mut body = auth.into_bytes();
            body.push(b'|');
            body.extend_from_slice(&message.payload);
            let _ = client.publish(reply, Bytes::from(body)).await;
        }
    });
    Ok(())
}

#[tokio::test]
async fn a_plain_request_returns_the_reply() -> TestResult {
    let (_server, client) = started().await?;
    spawn_echo(client.clone(), "echo.plain").await?;
    let bus = NatsBus::new(client);
    let reply = tokio::time::timeout(STEP, bus.request("echo.plain", Bytes::from("hi"), STEP, None)).await??;
    assert_eq!(reply, Some(Bytes::from("none|hi")));
    Ok(())
}

#[tokio::test]
async fn headers_travel_with_the_request() -> TestResult {
    let (_server, client) = started().await?;
    spawn_echo(client.clone(), "echo.headers").await?;
    let bus = NatsBus::new(client);
    let mut headers = HeaderMap::new();
    headers.insert("Authorization", "Bearer k");
    let reply = tokio::time::timeout(
        STEP,
        bus.request("echo.headers", Bytes::from("hi"), STEP, Some(headers)),
    )
    .await??;
    assert_eq!(reply, Some(Bytes::from("Bearer k|hi")));
    Ok(())
}

#[tokio::test]
async fn a_silent_responder_is_a_timeout_not_an_error() -> TestResult {
    let (_server, client) = started().await?;
    let mut silent = client.subscribe("silent.plain").await?;
    client.flush().await?;
    tokio::spawn(async move { while silent.next().await.is_some() {} });
    let bus = NatsBus::new(client);
    let reply = tokio::time::timeout(
        STEP,
        bus.request("silent.plain", Bytes::new(), Duration::from_millis(200), None),
    )
    .await??;
    assert_eq!(reply, None);
    Ok(())
}

#[tokio::test]
async fn a_request_nobody_serves_is_a_bus_error() -> TestResult {
    let (_server, client) = started().await?;
    let bus = NatsBus::new(client);
    let result = tokio::time::timeout(STEP, bus.request("nobody.home", Bytes::new(), STEP, None)).await?;
    let error = result.err().ok_or("expected an error")?;
    assert!(matches!(error, BusError::Request(_)));
    assert!(error.to_string().starts_with("NATS request error"));
    Ok(())
}

#[tokio::test]
async fn a_subscription_streams_published_messages() -> TestResult {
    let (_server, client) = started().await?;
    let bus = NatsBus::new(client.clone());
    let mut stream = bus.subscribe("events.one").await?;
    client.flush().await?;
    client.publish("events.one", Bytes::from("a")).await?;
    client.publish("events.one", Bytes::from("b")).await?;
    let first = tokio::time::timeout(STEP, stream.next()).await?.ok_or("no first")?;
    let second = tokio::time::timeout(STEP, stream.next()).await?.ok_or("no second")?;
    assert_eq!(first.subject, "events.one");
    assert_eq!(first.payload, Bytes::from("a"));
    assert_eq!(second.payload, Bytes::from("b"));
    Ok(())
}

#[tokio::test]
async fn an_invalid_subscription_subject_is_a_bus_error() -> TestResult {
    let (_server, client) = started().await?;
    let bus = NatsBus::new(client);
    let error = bus.subscribe("").await.err().ok_or("expected an error")?;
    assert!(matches!(error, BusError::Subscribe(_)));
    assert!(error.to_string().starts_with("NATS subscribe error"));
    Ok(())
}

/// Serves `subject` through the chunked transfer protocol, echoing the request body.
async fn spawn_transfer_echo(client: Client, subject: &'static str) -> TestResult {
    let mut requests = client.subscribe(subject).await?;
    client.flush().await?;
    tokio::spawn(async move {
        while let Some(message) = requests.next().await {
            let Some(reply) = message.reply.clone() else {
                continue;
            };
            let client = client.clone();
            tokio::spawn(async move {
                if let Ok(body) = receive(&client, &message).await {
                    let _ = respond(&client, &reply, body).await;
                }
            });
        }
    });
    Ok(())
}

#[tokio::test]
async fn file_subjects_use_the_chunked_transfer() -> TestResult {
    let (_server, client) = started().await?;
    let subjects = [
        "sandbox.lease.write_files.lease-1",
        "sandbox.lease.read_file.lease-1",
        "sandbox.lease.file_op.lease-1",
    ];
    let bus = NatsBus::new(client.clone());
    for subject in subjects {
        spawn_transfer_echo(client.clone(), subject).await?;
        let body = Bytes::from((0..300 * 1024).map(|i| (i % 251) as u8).collect::<Vec<u8>>());
        let reply = tokio::time::timeout(STEP, bus.request(subject, body.clone(), STEP, None)).await??;
        assert_eq!(reply, Some(body), "{subject}");
    }
    Ok(())
}

#[tokio::test]
async fn a_silent_file_responder_is_a_timeout_not_an_error() -> TestResult {
    let (_server, client) = started().await?;
    let mut silent = client.subscribe("sandbox.lease.file_op.silent").await?;
    client.flush().await?;
    tokio::spawn(async move { while silent.next().await.is_some() {} });
    let bus = NatsBus::new(client);
    let reply = tokio::time::timeout(
        STEP,
        bus.request(
            "sandbox.lease.file_op.silent",
            Bytes::new(),
            Duration::from_millis(200),
            None,
        ),
    )
    .await??;
    assert_eq!(reply, None);
    Ok(())
}

#[tokio::test]
async fn a_file_request_nobody_serves_is_a_transfer_error() -> TestResult {
    let (_server, client) = started().await?;
    let bus = NatsBus::new(client);
    let result = tokio::time::timeout(
        STEP,
        bus.request("sandbox.lease.read_file.nobody", Bytes::new(), STEP, None),
    )
    .await?;
    let error = result.err().ok_or("expected an error")?;
    assert!(matches!(error, BusError::Transfer(NatsTransferError::Request(_))));
    assert!(error.to_string().starts_with("NATS transfer error"));
    Ok(())
}

#[tokio::test]
async fn connect_reaches_a_running_server() -> TestResult {
    let server = CoreTestServer::start().await;
    let client = SandboxClient::connect(&url(&server)).await?;
    let mut released = client.evidence_stream(&LeaseId::new("lease-1")).await?;
    let raw = async_nats::connect(url(&server)).await?;
    raw.flush().await?;
    raw.publish("sandbox.evidence.lease-1.lifecycle", Bytes::from("x"))
        .await?;
    let frame = tokio::time::timeout(STEP, released.next()).await?.ok_or("no frame")??;
    assert_eq!(frame.subject_suffix, "lifecycle");
    Ok(())
}

#[tokio::test]
async fn connect_fails_for_an_unreachable_server() -> TestResult {
    let result = SandboxClient::connect("nats://127.0.0.1:1").await;
    assert!(result.is_err());
    Ok(())
}

#[tokio::test]
async fn a_client_over_nats_sends_its_key_and_decodes_the_reply() -> TestResult {
    let (_server, client) = started().await?;
    let mut requests = client.subscribe("sandbox.lease.terminal.lease-1").await?;
    client.flush().await?;
    let responder = client.clone();
    tokio::spawn(async move {
        while let Some(message) = requests.next().await {
            let Some(reply) = message.reply.clone() else {
                continue;
            };
            let authorized = message
                .headers
                .as_ref()
                .and_then(|headers| headers.get("Authorization"))
                .is_some_and(|value| value.as_str() == "Bearer k");
            let body = if authorized {
                r#"{"outcome":"done"}"#
            } else {
                r#"{"outcome":"unsupported"}"#
            };
            let _ = responder.publish(reply, Bytes::from(body)).await;
        }
    });
    let sdk = SandboxClient::from_client(client).with_api_key("k".to_string());
    let response = tokio::time::timeout(
        STEP,
        sdk.terminal(&LeaseId::new("lease-1"), TerminalId::new("t-1")?, TerminalOp::Close),
    )
    .await??;
    assert_eq!(response, TerminalResponse::Done);
    Ok(())
}

#[tokio::test]
async fn a_client_over_nats_surfaces_a_missing_responder_as_a_transport_error() -> TestResult {
    let (_server, client) = started().await?;
    let sdk = SandboxClient::from_client(client);
    let result = tokio::time::timeout(
        STEP,
        sdk.terminal(&LeaseId::new("lease-9"), TerminalId::new("t-1")?, TerminalOp::Close),
    )
    .await?;
    assert!(matches!(result, Err(ClientError::Transport(_))));
    Ok(())
}
