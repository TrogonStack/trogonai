use std::error::Error;
use std::time::Duration;

use async_nats::Client;
use bytes::Bytes;
use futures::StreamExt;

use super::*;
use crate::test_support::LocalNats;

type TestResult = Result<(), Box<dyn Error>>;

/// Replies to every request on `subject` with `reply`, and reports whether
/// the request carried the auth header.
async fn spawn_responder(
    client: &Client,
    subject: &str,
    reply: &'static [u8],
) -> Result<tokio::sync::mpsc::Receiver<bool>, Box<dyn Error>> {
    let mut sub = client.subscribe(subject.to_owned()).await?;
    let (tx, rx) = tokio::sync::mpsc::channel(8);
    let responder = client.clone();
    tokio::spawn(async move {
        while let Some(message) = sub.next().await {
            let authed = message
                .headers
                .as_ref()
                .is_some_and(|h| h.get(trogon_sandbox_protocol::AUTH_HEADER).is_some());
            let _ = tx.send(authed).await;
            if let Some(reply_to) = message.reply {
                let _ = responder.publish(reply_to, Bytes::from_static(reply)).await;
            }
        }
    });
    client.flush().await?;
    Ok(rx)
}

#[test]
fn file_transfer_subjects_are_recognised() {
    assert!(is_file_transfer("a.write_files.b"));
    assert!(is_file_transfer("a.read_file.b"));
    assert!(is_file_transfer("a.file_op.b"));
    assert!(!is_file_transfer("a.create.b"));
}

#[tokio::test]
async fn requests_reach_the_responder() -> TestResult {
    let server = LocalNats::start().await;
    let bus = NatsBus::new(server.client.clone());
    let _rx = spawn_responder(&server.client, "t.plain", b"pong").await?;
    assert_eq!(bus.request("t.plain", Bytes::from_static(b"ping")).await?, "pong");
    assert_eq!(
        bus.request_with_timeout("t.plain", Bytes::new(), Duration::from_secs(5))
            .await?,
        "pong"
    );
    Ok(())
}

#[tokio::test]
async fn file_transfer_requests_use_the_transfer_protocol() -> TestResult {
    let server = LocalNats::start().await;
    let bus = NatsBus::new(server.client.clone());
    let _rx = spawn_responder(&server.client, "t.read_file.x", b"data").await?;
    assert_eq!(bus.request("t.read_file.x", Bytes::new()).await?, "data");
    assert_eq!(
        bus.request_with_auth("t.read_file.x", "signed", Bytes::new(), None)
            .await?,
        "data"
    );
    Ok(())
}

#[tokio::test]
async fn auth_requests_carry_the_header() -> TestResult {
    let server = LocalNats::start().await;
    let bus = NatsBus::new(server.client.clone());
    let mut rx = spawn_responder(&server.client, "t.auth", b"ok").await?;
    assert_eq!(
        bus.request_with_auth("t.auth", "signed", Bytes::new(), None).await?,
        "ok"
    );
    assert_eq!(rx.recv().await, Some(true));
    assert_eq!(
        bus.request_with_auth("t.auth", "signed", Bytes::new(), Some(Duration::from_secs(5)))
            .await?,
        "ok"
    );
    assert_eq!(rx.recv().await, Some(true));
    Ok(())
}

#[tokio::test]
async fn publishes_arrive_with_their_headers() -> TestResult {
    let server = LocalNats::start().await;
    let bus = NatsBus::new(server.client.clone());
    let mut sub = server.client.subscribe("t.events").await?;
    server.client.flush().await?;
    bus.publish("t.events", Bytes::from_static(b"one")).await?;
    bus.publish_with_msg_id("t.events", "id-1", Bytes::from_static(b"two"))
        .await?;
    bus.publish_with_auth("t.events", "signed", Bytes::from_static(b"three"))
        .await?;
    let one = sub.next().await.ok_or("no first message")?;
    assert_eq!(one.payload, "one");
    let two = sub.next().await.ok_or("no second message")?;
    let id = two
        .headers
        .as_ref()
        .and_then(|h| h.get(async_nats::header::NATS_MESSAGE_ID))
        .map(|v| v.as_str().to_owned());
    assert_eq!(id.as_deref(), Some("id-1"));
    let three = sub.next().await.ok_or("no third message")?;
    assert!(
        three
            .headers
            .as_ref()
            .is_some_and(|h| h.get(trogon_sandbox_protocol::AUTH_HEADER).is_some())
    );
    Ok(())
}

#[tokio::test]
async fn max_payload_comes_from_the_server() -> TestResult {
    let server = LocalNats::start().await;
    let bus = NatsBus::new(server.client.clone());
    assert!(bus.max_payload().is_some_and(|max| max > 0));
    Ok(())
}

#[tokio::test]
async fn a_request_without_responder_fails() -> TestResult {
    let server = LocalNats::start().await;
    let bus = NatsBus::new(server.client.clone());
    assert!(bus.request("t.nobody", Bytes::new()).await.is_err());
    Ok(())
}
