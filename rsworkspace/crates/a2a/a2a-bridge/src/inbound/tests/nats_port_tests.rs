use axum::body::{Body, to_bytes};
use axum::http::Request;
use tower::ServiceExt;
use trogon_nats::test_support::{CoreTestServer, JetStreamTestServer};

use super::*;

const WAIT: Duration = Duration::from_secs(5);

async fn caller_jwt() -> BridgeUserJwt {
    StubAuthCalloutMint::fixture()
        .unwrap()
        .mint(&CallerHttpsAuth::new("Bearer fixture"))
        .await
        .unwrap()
}

#[tokio::test]
async fn unary_port_preserves_headers_payload_and_gateway_reply() {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await.unwrap();
    let mut gateway = client.subscribe("a2a.v1.gateway.planner.message.send").await.unwrap();
    client.flush().await.unwrap();
    let port = AsyncNatsTokenGatewayUnary::from_single_url(server.address(), WAIT);
    let jwt = caller_jwt().await;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, "caller-request");
    let expected = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"caller-request","result":{}}"#);
    let (reply, ()) = tokio::join!(
        port.unary_request_gateway(
            &jwt,
            "a2a.v1.gateway.planner.message.send",
            headers,
            Bytes::from_static(b"request")
        ),
        async {
            let request = tokio::time::timeout(WAIT, gateway.next()).await.unwrap().unwrap();
            assert_eq!(request.payload.as_ref(), b"request");
            assert_eq!(
                request.headers.unwrap().get(REQ_ID_HEADER).unwrap().as_str(),
                "caller-request"
            );
            client.publish(request.reply.unwrap(), expected.clone()).await.unwrap();
        },
    );
    assert_eq!(reply.unwrap(), expected);
}

#[tokio::test]
async fn unary_port_bounds_a_gateway_request_that_never_replies() {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await.unwrap();
    let mut gateway = client.subscribe("gateway.stalled").await.unwrap();
    client.flush().await.unwrap();
    let port = AsyncNatsTokenGatewayUnary::new(vec![server.address().to_owned()], Duration::from_millis(100));
    let jwt = caller_jwt().await;
    let (reply, request) = tokio::join!(
        port.unary_request_gateway(&jwt, "gateway.stalled", async_nats::HeaderMap::new(), Bytes::new()),
        tokio::time::timeout(WAIT, gateway.next()),
    );
    assert!(request.unwrap().unwrap().reply.is_some());
    assert!(matches!(reply, Err(BridgeError::NatsPublish(_))));
}

#[tokio::test]
async fn unary_port_reports_no_responder_and_invalid_connection_configuration() {
    let server = CoreTestServer::start().await;
    let jwt = caller_jwt().await;
    for port in [
        AsyncNatsTokenGatewayUnary::from_single_url(server.address(), WAIT),
        AsyncNatsTokenGatewayUnary::new(Vec::new(), Duration::from_millis(100)),
    ] {
        let error = port
            .unary_request_gateway(&jwt, "gateway.missing", async_nats::HeaderMap::new(), Bytes::new())
            .await;
        assert!(matches!(error, Err(BridgeError::NatsPublish(_))));
    }
}

async fn publish_event(js: &async_nats::jetstream::Context, request: Option<&str>, body: &'static [u8]) {
    let mut headers = async_nats::HeaderMap::new();
    if let Some(request) = request {
        headers.insert(REQ_ID_HEADER, request);
    }
    js.publish_with_headers("a2a.v1.tasks.task-1.events", headers, Bytes::from_static(body))
        .await
        .unwrap()
        .await
        .unwrap();
}

#[tokio::test]
async fn live_task_stream_forwards_only_events_for_its_request_and_acknowledges_other_requests() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    let stream = js
        .create_stream(a2a_nats::nats::subjects::A2aStream::Events.config(&default_a2a_prefix()))
        .await
        .unwrap();
    publish_event(&js, Some("other-request"), b"other").await;
    publish_event(&js, None, b"uncorrelated").await;
    publish_event(&js, Some("my-request"), b"mine").await;
    let port = AsyncNatsTokenTaskJetstream::from_single_url(server.address(), WAIT);
    let mut payloads = port
        .task_event_payload_stream(
            &caller_jwt().await,
            &default_a2a_prefix(),
            SseConsumePlan::MessageStreamBootstrap {
                task_id: A2aTaskId::new("task-1").unwrap(),
                req_id: ReqId::from_header("my-request"),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(WAIT, payloads.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .as_ref(),
        b"mine"
    );

    let name = stream.consumer_names().next().await.unwrap().unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            let info = stream.consumer_info(&name).await.unwrap();
            if info.ack_floor.stream_sequence == 3 && info.num_ack_pending == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), payloads.next())
            .await
            .is_err()
    );
    drop(payloads);
}

#[tokio::test]
async fn resumed_task_stream_replays_after_the_checkpoint_without_request_demultiplexing() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    js.create_stream(a2a_nats::nats::subjects::A2aStream::Events.config(&default_a2a_prefix()))
        .await
        .unwrap();
    publish_event(&js, Some("request-1"), b"before-checkpoint").await;
    publish_event(&js, Some("request-2"), b"after-checkpoint").await;
    publish_event(&js, None, b"uncorrelated-after-checkpoint").await;
    let port = AsyncNatsTokenTaskJetstream::new(vec![server.address().to_owned()], WAIT);
    let mut payloads = port
        .task_event_payload_stream(
            &caller_jwt().await,
            &default_a2a_prefix(),
            SseConsumePlan::TasksResubscribe {
                task_id: A2aTaskId::new("task-1").unwrap(),
                last_seq: 1,
            },
        )
        .await
        .unwrap();
    for expected in [b"after-checkpoint".as_slice(), b"uncorrelated-after-checkpoint"] {
        assert_eq!(
            tokio::time::timeout(WAIT, payloads.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .as_ref(),
            expected
        );
    }
    drop(payloads);
}

#[tokio::test]
async fn task_stream_reports_missing_stream_and_invalid_connection_configuration() {
    let server = JetStreamTestServer::start().await;
    for port in [
        AsyncNatsTokenTaskJetstream::from_single_url(server.address(), WAIT),
        AsyncNatsTokenTaskJetstream::new(Vec::new(), Duration::from_millis(100)),
    ] {
        let result = port
            .task_event_payload_stream(
                &caller_jwt().await,
                &default_a2a_prefix(),
                SseConsumePlan::TasksResubscribe {
                    task_id: A2aTaskId::new("task-1").unwrap(),
                    last_seq: 0,
                },
            )
            .await;
        assert!(matches!(result, Err(BridgeError::JetStreamConsume(_))));
    }
}

struct FixedPublisher(Bytes);

#[async_trait]
impl InboundGatewayPublish for FixedPublisher {
    async fn publish_unary_to_gateway(
        &self,
        _subject: &str,
        _jwt: &BridgeUserJwt,
        _headers: async_nats::HeaderMap,
        _payload: &[u8],
    ) -> Result<Bytes, BridgeError> {
        Ok(self.0.clone())
    }
}

fn http_request(body: Bytes) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri("/")
        .body(Body::from(body))
        .unwrap();
    *request.headers_mut() = caller_headers("planner", None);
    request
}

#[tokio::test]
async fn router_returns_transport_failures_as_bad_gateway_json() {
    let app = gateway_router(test_state(Arc::new(StubInboundGatewayPublish)));
    let response = app
        .oneshot(http_request(Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":1,"method":"message/send","params":{}}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(body["error"].as_str().unwrap().contains("gateway publish"));
}

#[tokio::test]
async fn router_preserves_gateway_jsonrpc_errors_for_streaming_requests() {
    let reply =
        Bytes::from_static(br#"{"jsonrpc":"2.0","id":"caller-request","error":{"code":-32602,"message":"rejected"}}"#);
    let app = gateway_router(test_state(Arc::new(FixedPublisher(reply.clone()))));
    let response = app
        .oneshot(http_request(Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":"caller-request","method":"message/stream","params":{}}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/json"
    );
    assert_eq!(to_bytes(response.into_body(), usize::MAX).await.unwrap(), reply);
}

#[tokio::test]
async fn router_finishes_a_message_only_stream_without_opening_a_task_consumer() {
    let state = AppState::new(
        Arc::new(StubAuthCalloutMint::fixture().unwrap()),
        Arc::new(FixedPublisher(bootstrap_bare_message().into())),
        Arc::new(StubTaskJetStreamPort),
        default_a2a_prefix(),
    );
    let response = gateway_router(state)
        .oneshot(http_request(Bytes::from_static(
            br#"{"jsonrpc":"2.0","id":"caller-request","method":"message/stream","params":{}}"#,
        )))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = tokio::time::timeout(WAIT, to_bytes(response.into_body(), usize::MAX))
        .await
        .unwrap()
        .unwrap();
    let text = std::str::from_utf8(&body).unwrap();
    let frames: Vec<Value> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|data| serde_json::from_str(data.trim()).unwrap())
        .collect();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["id"], "caller-request");
    assert!(frames[0]["result"]["message"].is_object());
}
