use std::time::Duration;

use futures::StreamExt;
use serde::{Serialize, Serializer};
use trogon_nats::test_support::JetStreamTestServer;

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

struct Unencodable;

impl Serialize for Unencodable {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("not encodable"))
    }
}

#[tokio::test]
async fn a_value_is_published_as_json_on_the_reply_subject() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut replies = client.subscribe("reply.json").await?;
    client.flush().await?;
    publish_json(&client, Subject::from("reply.json"), &vec![1, 2, 3], "test").await;
    let message = tokio::time::timeout(Duration::from_secs(5), replies.next())
        .await?
        .ok_or("no reply arrived")?;
    assert_eq!(message.payload.as_ref(), b"[1,2,3]");
    Ok(())
}

#[tokio::test]
async fn a_value_that_cannot_be_encoded_publishes_nothing() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut replies = client.subscribe("reply.unencodable").await?;
    client.flush().await?;
    publish_json(&client, Subject::from("reply.unencodable"), &Unencodable, "test").await;
    client.flush().await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), replies.next())
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test]
async fn a_body_larger_than_the_server_accepts_publishes_nothing() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut replies = client.subscribe("reply.oversize").await?;
    client.flush().await?;
    let body = Bytes::from(vec![0; client.server_info().max_payload + 1]);
    publish_body(&client, Subject::from("reply.oversize"), body, "test").await;
    client.flush().await?;
    assert!(
        tokio::time::timeout(Duration::from_millis(300), replies.next())
            .await
            .is_err()
    );
    Ok(())
}
