use a2a_nats::nats::subjects::tasks::TaskEventsSubject;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use tower::ServiceExt;
use trogon_nats::test_support::{CoreTestServer, JetStreamTestServer, TlsTestServer};
use trogon_nats::{Location, NatsTls};

use super::*;

const WAIT: Duration = Duration::from_secs(5);
const BRIEF: Duration = Duration::from_millis(100);
const CONNECT: CallerConnectTimeout = CallerConnectTimeout::new(WAIT);
const BRIEF_CONNECT: CallerConnectTimeout = CallerConnectTimeout::new(BRIEF);
const RPC: GatewayRpcTimeout = GatewayRpcTimeout::new(WAIT);
const BRIEF_RPC: GatewayRpcTimeout = GatewayRpcTimeout::new(BRIEF);
const UNREACHABLE_RPC: GatewayRpcTimeout = GatewayRpcTimeout::new(Duration::from_secs(3600));
const REFUSING_SERVER: &str = "127.0.0.1:1";

fn plaintext(address: &str) -> NatsConfig {
    NatsConfig::from_url(address).with_tls(NatsTls::Disabled)
}

fn no_servers() -> NatsConfig {
    NatsConfig::new(Vec::new(), NatsAuth::None)
}

struct SilentServer {
    address: String,
    accepting: tokio::task::JoinHandle<()>,
}

impl SilentServer {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let accepting = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((connection, _)) = listener.accept().await {
                held.push(connection);
            }
        });
        Self { address, accepting }
    }
}

impl Drop for SilentServer {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}

async fn gateway_client(config: &NatsConfig) -> async_nats::Client {
    trogon_nats::connect(config, WAIT).await.unwrap()
}

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
    let client = gateway_client(&plaintext(server.address())).await;
    let mut gateway = client
        .subscribe("a2a.v1.gateway.planner.person.harness.message.send")
        .await
        .unwrap();
    client.flush().await.unwrap();
    let port = AsyncNatsTokenGatewayUnary::new(plaintext(server.address()), CONNECT, RPC);
    let jwt = caller_jwt().await;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(REQ_ID_HEADER, "caller-request");
    let expected = Bytes::from_static(br#"{"jsonrpc":"2.0","id":"caller-request","result":{}}"#);
    let (reply, ()) = tokio::join!(
        port.unary_request_gateway(
            &jwt,
            "a2a.v1.gateway.planner.person.harness.message.send",
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
    let client = gateway_client(&plaintext(server.address())).await;
    let mut gateway = client.subscribe("gateway.stalled").await.unwrap();
    client.flush().await.unwrap();
    let port = AsyncNatsTokenGatewayUnary::new(plaintext(server.address()), CONNECT, BRIEF_RPC);
    let jwt = caller_jwt().await;
    let (reply, request) = tokio::join!(
        port.unary_request_gateway(&jwt, "gateway.stalled", async_nats::HeaderMap::new(), Bytes::new()),
        tokio::time::timeout(WAIT, gateway.next()),
    );
    assert!(request.unwrap().unwrap().reply.is_some());
    assert!(matches!(reply, Err(BridgeError::NatsPublish(_))));
}

#[tokio::test]
async fn caller_connections_give_up_on_a_silent_server_at_the_connect_timeout_rather_than_the_rpc_timeout() {
    let silent = SilentServer::start().await;
    let jwt = caller_jwt().await;
    let connect = CallerConnectTimeout::new(Duration::from_millis(200));
    let unary = AsyncNatsTokenGatewayUnary::new(plaintext(&silent.address), connect, UNREACHABLE_RPC);
    let jetstream = AsyncNatsTokenTaskJetstream::new(plaintext(&silent.address), connect);
    let prefix = default_a2a_prefix();

    let (reply, stream) = tokio::time::timeout(WAIT, async {
        tokio::join!(
            unary.unary_request_gateway(&jwt, "gateway.silent", async_nats::HeaderMap::new(), Bytes::new()),
            jetstream.task_event_payload_stream(
                &jwt,
                &prefix,
                SseConsumePlan::TasksResubscribe {
                    task_id: A2aTaskId::new("task-1").unwrap(),
                    last_seq: 0,
                },
            ),
        )
    })
    .await
    .expect("only the connect timeout may bound a caller connection that the server never answers");

    assert!(
        matches!(&reply, Err(BridgeError::NatsPublish(message)) if !message.contains("gateway RPC exceeded")),
        "{reply:?}"
    );
    assert!(matches!(stream, Err(BridgeError::JetStreamConsume(_))));
}

#[tokio::test]
async fn unary_port_reports_no_responder_and_invalid_connection_configuration() {
    let server = CoreTestServer::start().await;
    let jwt = caller_jwt().await;
    for port in [
        AsyncNatsTokenGatewayUnary::new(plaintext(server.address()), CONNECT, RPC),
        AsyncNatsTokenGatewayUnary::new(no_servers(), BRIEF_CONNECT, BRIEF_RPC),
    ] {
        let error = port
            .unary_request_gateway(&jwt, "gateway.missing", async_nats::HeaderMap::new(), Bytes::new())
            .await;
        assert!(matches!(error, Err(BridgeError::NatsPublish(_))));
    }
}

#[tokio::test]
async fn caller_connection_failures_do_not_tell_the_http_caller_which_servers_were_tried() {
    let jwt = caller_jwt().await;
    let refusing = plaintext(REFUSING_SERVER);

    let reply = AsyncNatsTokenGatewayUnary::new(refusing.clone(), CONNECT, RPC)
        .unary_request_gateway(&jwt, "gateway.refused", async_nats::HeaderMap::new(), Bytes::new())
        .await;
    let stream = AsyncNatsTokenTaskJetstream::new(refusing, CONNECT)
        .task_event_payload_stream(
            &jwt,
            &default_a2a_prefix(),
            SseConsumePlan::TasksResubscribe {
                task_id: A2aTaskId::new("task-1").unwrap(),
                last_seq: 0,
            },
        )
        .await;

    let errors = [
        reply.expect_err("a refused connection fails the unary request"),
        stream.err().expect("a refused connection fails the stream attach"),
    ];
    for error in errors {
        let (BridgeError::NatsPublish(message) | BridgeError::JetStreamConsume(message)) = &error else {
            panic!("expected a NATS failure, got {error:?}");
        };
        assert!(!message.contains(REFUSING_SERVER), "{message}");
    }
}

#[tokio::test]
async fn unary_port_authenticates_as_the_caller_rather_than_as_the_bridge() {
    let jwt = caller_jwt().await;
    let server = CoreTestServer::start_requiring_token(jwt.as_str()).await;
    let token = |secret: &str| NatsAuth::Token(CredentialValue::Literal(SecretString::new(secret).unwrap()));
    let responder = gateway_client(
        &NatsConfig::new(vec![server.address().to_owned()], token(jwt.as_str())).with_tls(NatsTls::Disabled),
    )
    .await;
    let mut gateway = responder.subscribe("gateway.guarded").await.unwrap();
    responder.flush().await.unwrap();
    let bridge =
        NatsConfig::new(vec![server.address().to_owned()], token("bridge-service-token")).with_tls(NatsTls::Disabled);
    let port = AsyncNatsTokenGatewayUnary::new(bridge, CONNECT, RPC);
    let (reply, ()) = tokio::join!(
        port.unary_request_gateway(&jwt, "gateway.guarded", async_nats::HeaderMap::new(), Bytes::new()),
        async {
            let request = tokio::time::timeout(WAIT, gateway.next()).await.unwrap().unwrap();
            responder
                .publish(request.reply.unwrap(), Bytes::from_static(b"granted"))
                .await
                .unwrap();
        },
    );
    assert_eq!(reply.unwrap(), Bytes::from_static(b"granted"));

    let stranger = BridgeUserJwt::new("other.caller.jwt").unwrap();
    let rejected = port
        .unary_request_gateway(&stranger, "gateway.guarded", async_nats::HeaderMap::new(), Bytes::new())
        .await;
    assert!(
        matches!(&rejected, Err(BridgeError::NatsPublish(message)) if message.contains("authorization violation")),
        "{rejected:?}"
    );
}

#[tokio::test]
async fn caller_connections_refuse_a_plaintext_server_unless_tls_is_disabled() {
    let server = JetStreamTestServer::start().await;
    server
        .jetstream()
        .await
        .create_stream(a2a_nats::nats::subjects::A2aStream::Events.config(&default_a2a_prefix()))
        .await
        .unwrap();
    let responder = server.client().await;
    let mut gateway = responder.subscribe("gateway.guarded").await.unwrap();
    responder.flush().await.unwrap();
    let default_tls = NatsConfig::from_url(server.address());
    let jwt = caller_jwt().await;

    let reply = AsyncNatsTokenGatewayUnary::new(default_tls.clone(), CONNECT, RPC)
        .unary_request_gateway(&jwt, "gateway.guarded", async_nats::HeaderMap::new(), Bytes::new())
        .await;
    let stream = AsyncNatsTokenTaskJetstream::new(default_tls, CONNECT)
        .task_event_payload_stream(
            &jwt,
            &default_a2a_prefix(),
            SseConsumePlan::TasksResubscribe {
                task_id: A2aTaskId::new("task-1").unwrap(),
                last_seq: 0,
            },
        )
        .await;

    assert!(matches!(reply, Err(BridgeError::NatsPublish(_))));
    assert!(matches!(stream, Err(BridgeError::JetStreamConsume(_))));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), gateway.next())
            .await
            .is_err(),
        "no request may reach a server that was not reached over TLS"
    );
}

#[tokio::test]
async fn unary_port_verifies_the_server_against_the_configured_ca() {
    let server = TlsTestServer::start().await;
    let trusting = NatsConfig::from_url(server.address()).with_tls(NatsTls::Enabled {
        ca: Some(Location::Path(server.ca())),
        client_cert: None,
    });
    let responder = gateway_client(&trusting).await;
    let mut gateway = responder.subscribe("gateway.sealed").await.unwrap();
    responder.flush().await.unwrap();
    let port = AsyncNatsTokenGatewayUnary::new(trusting, CONNECT, RPC);
    let jwt = caller_jwt().await;
    let (reply, ()) = tokio::join!(
        port.unary_request_gateway(&jwt, "gateway.sealed", async_nats::HeaderMap::new(), Bytes::new()),
        async {
            let request = tokio::time::timeout(WAIT, gateway.next()).await.unwrap().unwrap();
            responder
                .publish(request.reply.unwrap(), Bytes::from_static(b"sealed"))
                .await
                .unwrap();
        },
    );
    assert_eq!(reply.unwrap(), Bytes::from_static(b"sealed"));
}

fn fixture_principal() -> SubjectPrincipal {
    SubjectPrincipal::from_tokens("person", "harness").unwrap()
}

async fn publish_event(js: &async_nats::jetstream::Context, request: Option<&str>, body: &'static [u8]) {
    publish_event_for(js, &fixture_principal(), request, body).await;
}

async fn publish_event_for(
    js: &async_nats::jetstream::Context,
    caller: &SubjectPrincipal,
    request: Option<&str>,
    body: &'static [u8],
) {
    let mut headers = async_nats::HeaderMap::new();
    if let Some(request) = request {
        headers.insert(REQ_ID_HEADER, request);
    }
    let subject = TaskEventsSubject::new(&default_a2a_prefix(), &A2aTaskId::new("task-1").unwrap(), caller);
    js.publish_with_headers(subject, headers, Bytes::from_static(body))
        .await
        .unwrap()
        .await
        .unwrap();
}

#[tokio::test]
async fn task_stream_leaves_out_events_published_for_another_principal_on_the_same_task() {
    let server = JetStreamTestServer::start().await;
    let js = server.jetstream().await;
    js.create_stream(a2a_nats::nats::subjects::A2aStream::Events.config(&default_a2a_prefix()))
        .await
        .unwrap();
    let other = SubjectPrincipal::from_tokens("person", "mallory").unwrap();
    publish_event_for(&js, &other, Some("my-request"), b"another-principal").await;
    publish_event(&js, Some("my-request"), b"mine").await;
    let port = AsyncNatsTokenTaskJetstream::new(plaintext(server.address()), CONNECT);
    let mut payloads = port
        .task_event_payload_stream(
            &caller_jwt().await,
            &default_a2a_prefix(),
            SseConsumePlan::TasksResubscribe {
                task_id: A2aTaskId::new("task-1").unwrap(),
                last_seq: 0,
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
    assert!(
        tokio::time::timeout(Duration::from_millis(100), payloads.next())
            .await
            .is_err()
    );
    drop(payloads);
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
    let port = AsyncNatsTokenTaskJetstream::new(plaintext(server.address()), CONNECT);
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
    let port = AsyncNatsTokenTaskJetstream::new(plaintext(server.address()), CONNECT);
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
        AsyncNatsTokenTaskJetstream::new(plaintext(server.address()), CONNECT),
        AsyncNatsTokenTaskJetstream::new(no_servers(), BRIEF_CONNECT),
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
        Arc::new(BearerOnlyGuard),
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
