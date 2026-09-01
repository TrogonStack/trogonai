//! ADR 0016 conformance: an Echo/Fail service registered through
//! [`grpc_nats_micro::serve`] against a real `nats-server` process.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::future::Future;
use std::net::TcpListener;
use std::pin::Pin;
use std::time::Duration;

use async_nats::HeaderMap;
use async_nats::service::Service;
use buffa::Enumeration as _;
use grpc_nats_micro::constants::HEADER_ERROR_CODE;
use grpc_nats_micro::{ContentType, EndpointHandler, ServiceBinding};
use tokio::process::{Child, Command};
use trogonai_proto::google::rpc::{Code, Status};
use trogonai_proto::grpc_nats_micro::v1::{EchoReply, EchoRequest, FailRequest};

const SUBJECT_PREFIX: &str = "echo.v1";
const SERVICE_NAME: &str = "EchoService";
const SERVICE_VERSION: &str = "0.1.0";
const SAY_METHOD: &str = "Say";
const FAIL_METHOD: &str = "Fail";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

struct NatsServerProcess {
    child: Child,
    port: u16,
}

impl NatsServerProcess {
    async fn spawn() -> Self {
        let port = free_port();
        let child = Command::new("nats-server")
            .args(["-p", &port.to_string(), "-a", "127.0.0.1"])
            .kill_on_drop(true)
            .spawn()
            .expect("spawn nats-server; is it on PATH?");
        let process = Self { child, port };
        process.wait_until_ready().await;
        process
    }

    async fn wait_until_ready(&self) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if async_nats::connect(self.url()).await.is_ok() {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!("nats-server did not become ready on port {}", self.port);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn url(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for NatsServerProcess {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

struct SayHandler;

impl EndpointHandler for SayHandler {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, Status>> + Send + 'a>> {
        Box::pin(async move {
            let request: EchoRequest = content_type.decode(request_bytes).map_err(|error| Status {
                code: Code::INVALID_ARGUMENT.to_i32(),
                message: error.to_string(),
                details: Vec::new(),
            })?;
            let reply = EchoReply {
                message: request.message,
            };
            content_type.encode(&reply).map_err(|error| Status {
                code: Code::INTERNAL.to_i32(),
                message: error.to_string(),
                details: Vec::new(),
            })
        })
    }
}

struct FailHandler;

impl EndpointHandler for FailHandler {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, Status>> + Send + 'a>> {
        Box::pin(async move {
            let request: FailRequest = content_type.decode(request_bytes).map_err(|error| Status {
                code: Code::INVALID_ARGUMENT.to_i32(),
                message: error.to_string(),
                details: Vec::new(),
            })?;
            let code = request.code.and_then(|value| value.as_known()).unwrap_or(Code::UNKNOWN);
            Err(Status {
                code: code.to_i32(),
                message: request.message.unwrap_or_default(),
                details: Vec::new(),
            })
        })
    }
}

fn echo_service_binding() -> ServiceBinding {
    ServiceBinding::new(SERVICE_NAME, SERVICE_VERSION, SUBJECT_PREFIX)
        .with_method(SAY_METHOD)
        .with_method(FAIL_METHOD)
}

/// Keeps the spawned `nats-server` process, the client, the service
/// registration, and the derived subject binding alive together: dropping
/// the [`Service`] handle closes its internal shutdown broadcast, which
/// stops every endpoint task started by [`grpc_nats_micro::serve`].
struct EchoFixture {
    _server: NatsServerProcess,
    client: async_nats::Client,
    binding: ServiceBinding,
    _service: Service,
}

async fn start_fixture() -> EchoFixture {
    let server = NatsServerProcess::spawn().await;
    let client = async_nats::connect(server.url()).await.expect("connect to nats-server");

    let binding = echo_service_binding();
    let handlers: Vec<Box<dyn EndpointHandler>> = vec![Box::new(SayHandler), Box::new(FailHandler)];

    let service = grpc_nats_micro::serve(
        &client,
        &binding,
        trogonai_proto::nats::micro::v1alpha1::ServiceOptions::default(),
        handlers,
    )
    .await
    .expect("start EchoService");

    EchoFixture {
        _server: server,
        client,
        binding,
        _service: service,
    }
}

async fn say(fixture: &EchoFixture, content_type: ContentType, message: &str) -> async_nats::Message {
    let endpoint = fixture
        .binding
        .endpoints()
        .iter()
        .find(|endpoint| endpoint.method_name() == SAY_METHOD)
        .expect("Say endpoint registered");

    let request = EchoRequest {
        message: Some(message.to_string()),
    };
    let body = content_type.encode(&request).expect("encode EchoRequest");
    let mut headers = HeaderMap::new();
    headers.insert(
        grpc_nats_micro::constants::HEADER_CONTENT_TYPE,
        content_type.header_value(),
    );

    let subject = endpoint.subject().as_str().to_string();
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        fixture.client.request_with_headers(subject, headers, body.into()),
    )
    .await
    .expect("Say request did not time out")
    .expect("Say request succeeded")
}

async fn fail(fixture: &EchoFixture, content_type: ContentType, code: Code, message: &str) -> async_nats::Message {
    let endpoint = fixture
        .binding
        .endpoints()
        .iter()
        .find(|endpoint| endpoint.method_name() == FAIL_METHOD)
        .expect("Fail endpoint registered");

    let request = FailRequest {
        code: Some(code.into()),
        message: Some(message.to_string()),
    };
    let body = content_type.encode(&request).expect("encode FailRequest");
    let mut headers = HeaderMap::new();
    headers.insert(
        grpc_nats_micro::constants::HEADER_CONTENT_TYPE,
        content_type.header_value(),
    );

    let subject = endpoint.subject().as_str().to_string();
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        fixture.client.request_with_headers(subject, headers, body.into()),
    )
    .await
    .expect("Fail request did not time out")
    .expect("Fail request succeeded")
}

async fn assert_say_round_trips(content_type: ContentType) {
    let fixture = start_fixture().await;

    let response = say(&fixture, content_type, "hello").await;

    assert!(
        response
            .headers
            .as_ref()
            .and_then(|headers| headers.get(HEADER_ERROR_CODE))
            .is_none(),
        "successful Say reply must not carry {HEADER_ERROR_CODE}"
    );
    let reply: EchoReply = content_type.decode(&response.payload).expect("decode EchoReply");
    assert_eq!(reply.message, Some("hello".to_string()));
}

async fn assert_fail_reports_status(content_type: ContentType) {
    let fixture = start_fixture().await;

    let response = fail(&fixture, content_type, Code::ALREADY_EXISTS, "already exists").await;

    let headers = response.headers.as_ref().expect("error reply carries headers");
    let error_code_header = headers
        .get(HEADER_ERROR_CODE)
        .expect("error reply must carry Nats-Service-Error-Code")
        .as_str();
    assert_eq!(error_code_header, Code::ALREADY_EXISTS.to_i32().to_string());

    let status: Status = content_type
        .decode(&response.payload)
        .expect("decode complete Status body");
    assert_eq!(status.code, Code::ALREADY_EXISTS.to_i32());
    assert_eq!(status.message, "already exists");
}

#[tokio::test]
async fn say_round_trips_over_protobuf() {
    assert_say_round_trips(ContentType::Protobuf).await;
}

#[tokio::test]
async fn say_round_trips_over_json() {
    assert_say_round_trips(ContentType::Json).await;
}

#[tokio::test]
async fn fail_reports_status_over_protobuf() {
    assert_fail_reports_status(ContentType::Protobuf).await;
}

#[tokio::test]
async fn fail_reports_status_over_json() {
    assert_fail_reports_status(ContentType::Json).await;
}
