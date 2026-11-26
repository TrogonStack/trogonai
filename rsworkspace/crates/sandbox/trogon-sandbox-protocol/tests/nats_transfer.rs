mod nats_common;

use std::time::Duration;

use async_nats::{HeaderMap, Message};
use bytes::Bytes;
use futures::StreamExt;
use trogon_sandbox_protocol::NatsTransferError;
use trogon_sandbox_protocol::nats_transfer::{MAX_TRANSFER_BYTES, receive, request, respond};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const TIMEOUT: Duration = Duration::from_secs(10);

fn pattern(len: usize) -> Bytes {
    Bytes::from((0..len).map(|i| (i % 251) as u8).collect::<Vec<u8>>())
}

fn message(headers: Option<HeaderMap>, payload: &'static [u8]) -> Result<Message, Box<dyn std::error::Error>> {
    Ok(Message {
        subject: "test.subject".into(),
        reply: None,
        payload: Bytes::from_static(payload),
        headers,
        status: None,
        description: None,
        length: payload.len(),
    })
}

/// Serves `subject` by echoing every request through the transfer protocol.
async fn spawn_echo(client: async_nats::Client, subject: &'static str) -> Result<(), Box<dyn std::error::Error>> {
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
async fn an_inline_payload_round_trips() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    spawn_echo(client.clone(), "echo.small").await?;
    let body = pattern(100);
    let reply = request(&client, "echo.small", body.clone(), None, TIMEOUT).await?;
    assert_eq!(reply, body);
    Ok(())
}

#[tokio::test]
async fn a_payload_over_one_message_is_chunked_both_ways() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    spawn_echo(client.clone(), "echo.large").await?;
    let body = pattern(200 * 1024 + 7);
    let mut headers = HeaderMap::new();
    headers.insert("X-Extra", "kept");
    let reply = request(&client, "echo.large", body.clone(), Some(headers), TIMEOUT).await?;
    assert_eq!(reply, body);
    Ok(())
}

#[tokio::test]
async fn a_payload_over_the_cap_is_refused() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let body = Bytes::from(vec![0u8; MAX_TRANSFER_BYTES + 1]);
    let error = request(&client, "nobody.home", body, None, TIMEOUT)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, NatsTransferError::TooLarge { max } if max == MAX_TRANSFER_BYTES));
    Ok(())
}

#[tokio::test]
async fn a_request_nobody_answers_fails() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let error = request(&client, "nobody.home", pattern(10), None, TIMEOUT)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, NatsTransferError::Request(_)));
    Ok(())
}

#[tokio::test]
async fn a_request_past_its_deadline_times_out() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let mut silent = client.subscribe("silent.subject").await?;
    client.flush().await?;
    tokio::spawn(async move { while silent.next().await.is_some() {} });
    let error = request(&client, "silent.subject", pattern(10), None, Duration::from_millis(100))
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(
        matches!(
            error,
            NatsTransferError::RequestTimedOut | NatsTransferError::Request(_)
        ),
        "{error}"
    );
    Ok(())
}

#[tokio::test]
async fn a_message_without_the_transfer_header_is_returned_inline() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let received = receive(&client, &message(None, b"inline")?).await?;
    assert_eq!(&received[..], b"inline");
    Ok(())
}

#[tokio::test]
async fn an_unknown_transfer_version_is_refused() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let mut headers = HeaderMap::new();
    headers.insert("Sandbox-Transfer", "chunks-v9");
    let error = receive(&client, &message(Some(headers), b"{}")?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, NatsTransferError::UnknownVersion));
    Ok(())
}

#[tokio::test]
async fn a_malformed_descriptor_is_refused() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let mut headers = HeaderMap::new();
    headers.insert("Sandbox-Transfer", "chunks-v1");
    let error = receive(&client, &message(Some(headers), b"nope")?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, NatsTransferError::Descriptor(_)));
    Ok(())
}

#[tokio::test]
async fn a_chunk_of_the_wrong_length_is_refused() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let mut chunks = client.subscribe("_INBOX.liar").await?;
    client.flush().await?;
    let responder = client.clone();
    tokio::spawn(async move {
        while let Some(chunk) = chunks.next().await {
            if let Some(reply) = chunk.reply {
                let _ = responder.publish(reply, Bytes::from_static(b"short")).await;
            }
        }
    });
    let mut headers = HeaderMap::new();
    headers.insert("Sandbox-Transfer", "chunks-v1");
    let descriptor = br#"{"inbox":"_INBOX.liar","length":100,"chunk_size":50}"#;
    let error = receive(&client, &message(Some(headers), descriptor)?)
        .await
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, NatsTransferError::InvalidChunkLength));
    Ok(())
}

#[tokio::test]
async fn respond_publishes_small_payloads_inline() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let mut replies = client.subscribe("reply.here").await?;
    client.flush().await?;
    respond(&client, "reply.here", Bytes::from_static(b"ok")).await?;
    let reply = tokio::time::timeout(TIMEOUT, replies.next()).await?.ok_or("no reply")?;
    assert_eq!(&reply.payload[..], b"ok");
    assert!(reply.headers.is_none());
    Ok(())
}

/// Starts `respond` for a payload that needs chunking and returns the descriptor message it published.
async fn start_large_response(
    client: &async_nats::Client,
    reply: &'static str,
    body: Bytes,
) -> Result<(tokio::task::JoinHandle<Result<(), NatsTransferError>>, Message), Box<dyn std::error::Error>> {
    let mut replies = client.subscribe(reply).await?;
    client.flush().await?;
    let responder = client.clone();
    let task = tokio::spawn(async move { respond(&responder, reply, body).await });
    let descriptor = tokio::time::timeout(TIMEOUT, replies.next())
        .await?
        .ok_or("no descriptor")?;
    Ok((task, descriptor))
}

#[tokio::test]
async fn the_chunk_sender_ignores_requests_it_cannot_serve_and_still_completes() -> TestResult {
    let server = nats_common::Server::start().await;
    let client = async_nats::connect(server.url()).await?;
    let body = pattern(200 * 1024 + 3);
    let (task, descriptor) = start_large_response(&client, "reply.junk", body.clone()).await?;
    let inbox = serde_json::from_slice::<serde_json::Value>(&descriptor.payload)?["inbox"]
        .as_str()
        .ok_or("descriptor inbox")?
        .to_owned();
    client.publish(inbox.clone(), Bytes::from_static(b"0")).await?;
    for payload in ["not-a-number", "999999999", "1"] {
        client
            .publish_with_reply(inbox.clone(), "_INBOX.unused", Bytes::from(payload))
            .await?;
    }
    client.flush().await?;
    assert_eq!(receive(&client, &descriptor).await?, body);
    tokio::time::timeout(TIMEOUT, task).await???;
    Ok(())
}

#[tokio::test]
async fn the_chunk_sender_ends_when_its_subscription_closes() -> TestResult {
    let server = nats_common::Server::start().await;
    let sender = async_nats::connect(server.url()).await?;
    let reader = async_nats::connect(server.url()).await?;
    let mut replies = reader.subscribe("reply.closed").await?;
    reader.flush().await?;
    let responder = sender.clone();
    let task = tokio::spawn(async move { respond(&responder, "reply.closed", pattern(200 * 1024)).await });
    tokio::time::timeout(TIMEOUT, replies.next())
        .await?
        .ok_or("no descriptor")?;
    sender.drain().await?;
    tokio::time::timeout(TIMEOUT, task).await???;
    Ok(())
}
