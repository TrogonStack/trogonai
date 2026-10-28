use std::time::Duration;

use bytes::Bytes;
use serde_json::json;
use tokio::net::{TcpListener, TcpStream};

use crate::constants::{ENV_GATEWAY_UNARY_DEADLINE_SECS, ENV_TIER3_REDACTION_ENABLED};

use super::fixture::{DispatchFixture, TestResult, receive, request};

#[tokio::test]
async fn a_closed_reply_connection_reports_the_failed_ingress_response() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let drained = async_nats::connect(fixture.broker_address()).await?;
    drained.drain().await.expect("start connection drain");
    tokio::time::timeout(Duration::from_secs(5), async {
        while drained.publish("drain.probe", Bytes::new()).await.is_ok() {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    fixture.client = drained;
    let events = trogon_std::log_capture::CapturedEvents::new();
    let _capture = events.install(trogon_std::log_capture::LevelFilter::DEBUG);
    let mut message = request("unknown.method", json!({}));
    message.subject = "a2a.v1.gateway.bot.unknown.method".into();
    fixture.dispatch(message).await;
    assert!(
        events
            .events()
            .iter()
            .any(|event| event.message() == Some("gateway failed to publish ingress error reply"))
    );
    let observer = async_nats::connect(fixture.broker_address()).await?;
    super::fixture::assert_empty(&observer, &mut fixture.agents, "a2a.v1.agents.barrier").await?;
    Ok(())
}

#[tokio::test]
async fn a_saturated_connection_queue_expires_without_forwarding_the_request() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    fixture.env.set(ENV_GATEWAY_UNARY_DEADLINE_SECS, "1");
    let backend = fixture.broker_address().to_owned();
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let delayed = async_nats::ConnectOptions::new()
        .retry_on_initial_connect()
        .client_capacity(1)
        .connection_timeout(Duration::from_secs(10))
        .connect(address.to_string())
        .await?;
    delayed.publish("queue.occupied", Bytes::new()).await?;
    fixture.client = delayed;

    // A handshake delayed beyond the request deadline keeps the one-slot
    // outbound queue full, then restores the route for the error and audit.
    let proxy = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let (mut downstream, _) = listener.accept().await.expect("gateway connection");
        let mut upstream = TcpStream::connect(backend).await.expect("test broker connection");
        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
    });
    tokio::time::timeout(
        Duration::from_secs(8),
        fixture.dispatch(request(
            "message.send",
            json!({"message": {"role": "user", "parts": []}}),
        )),
    )
    .await?;
    let denial = fixture.denied(-32800, json!("request-7")).await?;
    assert_eq!(
        denial["error"]["message"],
        "gateway publish deadline exceeded for message/send"
    );
    let audit = fixture.audit("err").await?;
    assert_eq!(audit["code"], -32800);
    assert_eq!(audit["caller_id"], "alice");
    assert_eq!(audit["method"], "message/send");
    assert_eq!(audit["req_id"], "request-7");
    fixture
        .dispatch(request("message.send", json!({"message": "recovered"})))
        .await;
    receive(&mut fixture.agents).await?;
    assert_eq!(fixture.audit("ok").await?["req_id"], "request-7");
    proxy.abort();
    let _ = proxy.await;
    Ok(())
}

#[tokio::test]
async fn enabled_redaction_without_changes_is_distinct_from_a_disabled_layer() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    fixture.env.set(ENV_TIER3_REDACTION_ENABLED, "true");
    let message = request("message.send", json!({"message": "public"}));
    let expected = message.payload.clone();
    fixture.dispatch(message).await;
    assert_eq!(receive(&mut fixture.agents).await?.payload, expected);
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["tier3_decision"], "allow");
    assert_eq!(audit["rules_fired"][2], "gateway.tier3.evaluated_allow");
    Ok(())
}
