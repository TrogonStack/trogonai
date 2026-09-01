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
use buffa_types::google::protobuf::Any;
use grpc_nats_micro::client::RequestError;
use grpc_nats_micro::constants::HEADER_ERROR_CODE;
use grpc_nats_micro::status_codec::ReplyError;
use grpc_nats_micro::{
    ContentType, EndpointHandler, MethodName, ServiceBinding, ServiceFault, ServiceName, SubjectPrefix,
};
use tokio::process::{Child, Command};
use trogon_nats::{NatsConfig, RequestClient};
use trogonai_proto::google::rpc::{Code, ErrorInfo, Status};
use trogonai_proto::grpc_nats_micro::v1::{FailRequest, FailResponse, SayRequest, SayResponse};
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

const SUBJECT_PREFIX: &str = "echo.v1";
const SERVICE_NAME: &str = "EchoService";
const SERVICE_VERSION: &str = "0.1.0";
const SERVICE_DESCRIPTION: &str = "Echoes what it is told";
const SAY_METHOD: &str = "Say";
const FAIL_METHOD: &str = "Fail";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const STOPPED_SERVICE_TIMEOUT: Duration = Duration::from_millis(500);
const STOPPED_SERVICE_SETTLE: Duration = Duration::from_millis(100);
const FAIL_DETAIL_REASON: &str = "ECHO_FAIL";
const FAIL_DETAIL_DOMAIN: &str = "grpc-nats-micro.conformance";

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
            if connect(&self.url()).await.is_ok() {
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

async fn connect(url: &str) -> Result<async_nats::Client, trogon_nats::ConnectError> {
    trogon_nats::connect(&NatsConfig::from_url(url), CONNECT_TIMEOUT).await
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
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move {
            let request: SayRequest = content_type
                .decode(request_bytes)
                .map_err(|error| ServiceFault::invalid_argument(error.to_string()))?;
            let reply = SayResponse {
                message: request.message,
            };
            content_type
                .encode(&reply)
                .map_err(|error| ServiceFault::internal(error.to_string()))
        })
    }
}

struct FailHandler;

impl EndpointHandler for FailHandler {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move {
            let request: FailRequest = content_type
                .decode(request_bytes)
                .map_err(|error| ServiceFault::invalid_argument(error.to_string()))?;
            let code = request.code.and_then(|value| value.as_known()).unwrap_or(Code::UNKNOWN);
            let detail = ErrorInfo {
                reason: FAIL_DETAIL_REASON.to_string(),
                domain: FAIL_DETAIL_DOMAIN.to_string(),
                ..Default::default()
            };
            Err(ServiceFault::new(Status {
                code: code.to_i32(),
                message: request.message.unwrap_or_default(),
                details: vec![Any::pack(&detail, ErrorInfo::TYPE_URL)],
            })
            .expect("FailRequest carries a service error code"))
        })
    }
}

fn echo_service_binding() -> ServiceBinding {
    ServiceBinding::new(
        ServiceName::new(SERVICE_NAME).expect("valid service name"),
        SERVICE_VERSION,
        SubjectPrefix::new(SUBJECT_PREFIX).expect("valid subject prefix"),
    )
    .with_description(SERVICE_DESCRIPTION)
    .with_method(method(SAY_METHOD))
    .expect("derive Say subject")
    .with_method(method(FAIL_METHOD))
    .expect("derive Fail subject")
}

fn method(name: &str) -> MethodName {
    MethodName::new(name).expect("valid method name")
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
    start_fixture_with_policy(ServiceOptions::default()).await
}

async fn start_fixture_with_policy(content_type_policy: ServiceOptions) -> EchoFixture {
    let server = NatsServerProcess::spawn().await;
    let client = connect(&server.url()).await.expect("connect to nats-server");

    let binding = echo_service_binding();
    let handlers: Vec<Box<dyn EndpointHandler>> = vec![Box::new(SayHandler), Box::new(FailHandler)];

    let service = grpc_nats_micro::serve(&client, &binding, content_type_policy, handlers)
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
        .find(|endpoint| endpoint.method_name().as_str() == SAY_METHOD)
        .expect("Say endpoint registered");

    let request = SayRequest {
        message: Some(message.to_string()),
    };
    let body = content_type.encode(&request).expect("encode SayRequest");
    let mut headers = HeaderMap::new();
    headers.insert(
        grpc_nats_micro::constants::HEADER_CONTENT_TYPE,
        content_type.header_value(),
    );

    let subject = endpoint.subject().as_str().to_string();
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        RequestClient::request_with_headers(&fixture.client, subject, headers, body.into()),
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
        .find(|endpoint| endpoint.method_name().as_str() == FAIL_METHOD)
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
        RequestClient::request_with_headers(&fixture.client, subject, headers, body.into()),
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
    let reply: SayResponse = content_type.decode(&response.payload).expect("decode SayResponse");
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
    assert_eq!(error_info(&status).reason, FAIL_DETAIL_REASON);
}

/// ADR 0016 §3 makes the body the only place `Status.details` is readable, so
/// the client decode path must surface them rather than reduce the fault to
/// code and message.
async fn assert_fail_details_reach_the_client(content_type: ContentType) {
    let fixture = start_fixture_with_policy(ServiceOptions::default()).await;
    let endpoint = endpoint(&fixture, FAIL_METHOD);

    let request = FailRequest {
        code: Some(Code::RESOURCE_EXHAUSTED.into()),
        message: Some("out of quota".to_string()),
    };
    let error = grpc_nats_micro::client::request::<_, FailRequest, FailResponse>(
        &fixture.client,
        endpoint,
        content_type,
        &request,
        REQUEST_TIMEOUT,
    )
    .await
    .expect_err("Fail must surface a service error");

    let service_error = service_error(error);
    assert_eq!(service_error.code().code(), Code::RESOURCE_EXHAUSTED);
    assert_eq!(service_error.message(), "out of quota");
    let detail = error_info(service_error.status());
    assert_eq!(detail.reason, FAIL_DETAIL_REASON);
    assert_eq!(detail.domain, FAIL_DETAIL_DOMAIN);
}

fn error_info(status: &Status) -> ErrorInfo {
    status
        .details
        .first()
        .expect("Status carries an error detail")
        .unpack_if::<ErrorInfo>(ErrorInfo::TYPE_URL)
        .expect("decode ErrorInfo detail")
        .expect("detail is an ErrorInfo")
}

fn service_error(
    error: grpc_nats_micro::client::RequestError<async_nats::client::RequestError>,
) -> grpc_nats_micro::ServiceError {
    match error {
        grpc_nats_micro::client::RequestError::Reply(ReplyError::Service(service_error)) => service_error,
        other => panic!("expected a micro service error, got {other:?}"),
    }
}

fn endpoint<'a>(fixture: &'a EchoFixture, method_name: &str) -> &'a grpc_nats_micro::EndpointBinding {
    fixture
        .binding
        .endpoints()
        .iter()
        .find(|endpoint| endpoint.method_name().as_str() == method_name)
        .expect("endpoint registered")
}

#[tokio::test]
async fn fail_details_reach_the_client_over_protobuf() {
    assert_fail_details_reach_the_client(ContentType::Protobuf).await;
}

#[tokio::test]
async fn fail_details_reach_the_client_over_json() {
    assert_fail_details_reach_the_client(ContentType::Json).await;
}

/// ADR 0016 §2: the endpoint name is the rpc method name, so discovery reports
/// the method rather than the subject micro would otherwise name it after.
#[tokio::test]
async fn discovery_names_endpoints_after_rpc_methods() {
    let fixture = start_fixture().await;

    let response = tokio::time::timeout(
        REQUEST_TIMEOUT,
        fixture
            .client
            .request(format!("$SRV.INFO.{SERVICE_NAME}"), bytes::Bytes::new()),
    )
    .await
    .expect("$SRV.INFO did not time out")
    .expect("$SRV.INFO responded");

    let info: serde_json::Value = serde_json::from_slice(&response.payload).expect("decode $SRV.INFO record");
    assert_eq!(info["description"].as_str(), Some(SERVICE_DESCRIPTION));
    let mut names: Vec<&str> = info["endpoints"]
        .as_array()
        .expect("$SRV.INFO carries endpoints")
        .iter()
        .map(|endpoint| endpoint["name"].as_str().expect("endpoint name is a string"))
        .collect();
    names.sort_unstable();
    assert_eq!(names, vec![FAIL_METHOD, SAY_METHOD]);
}

/// A caller the content-type policy turns away must be able to read the
/// rejection: encoding it in a type the caller does not speak would surface as
/// a decode failure instead of the policy's `Status`.
#[tokio::test]
async fn rejected_content_type_reports_the_policy_status() {
    let fixture = start_fixture_with_policy(ServiceOptions {
        content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
        ..Default::default()
    })
    .await;
    let endpoint = endpoint(&fixture, SAY_METHOD);

    let request = SayRequest {
        message: Some("hello".to_string()),
    };
    let error = grpc_nats_micro::client::request::<_, SayRequest, SayResponse>(
        &fixture.client,
        endpoint,
        ContentType::Json,
        &request,
        REQUEST_TIMEOUT,
    )
    .await
    .expect_err("a JSON caller must be rejected by a protobuf-only service");

    let service_error = service_error(error);
    assert_eq!(service_error.code().code(), Code::INVALID_ARGUMENT);
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

/// A handler list that does not line up with the binding's endpoints would be
/// silently truncated by `zip`, leaving declared methods unserved.
#[tokio::test]
async fn a_handler_count_mismatch_is_rejected_before_startup() {
    let server = NatsServerProcess::spawn().await;
    let client = connect(&server.url()).await.expect("connect to nats-server");
    let binding = echo_service_binding();

    let error = grpc_nats_micro::serve(
        &client,
        &binding,
        ServiceOptions::default(),
        vec![Box::new(SayHandler) as Box<dyn EndpointHandler>],
    )
    .await
    .expect_err("a short handler list must be rejected");

    assert!(matches!(
        error,
        grpc_nats_micro::ServeError::HandlerCount {
            endpoints: 2,
            handlers: 1
        }
    ));
}

/// Stopping the service unsubscribes its endpoints, which ends the dispatch
/// task each one runs on and leaves the subject without a responder.
#[tokio::test]
async fn stopping_the_service_leaves_its_subjects_without_a_responder() {
    let server = NatsServerProcess::spawn().await;
    let client = connect(&server.url()).await.expect("connect to nats-server");
    let binding = echo_service_binding();
    let handlers: Vec<Box<dyn EndpointHandler>> = vec![Box::new(SayHandler), Box::new(FailHandler)];
    let service = grpc_nats_micro::serve(&client, &binding, ServiceOptions::default(), handlers)
        .await
        .expect("start EchoService");

    service.stop().await.expect("stop EchoService");
    tokio::time::sleep(STOPPED_SERVICE_SETTLE).await;
    client.flush().await.expect("flush the unsubscribes");

    let endpoint = binding
        .endpoints()
        .iter()
        .find(|endpoint| endpoint.method_name().as_str() == SAY_METHOD)
        .expect("Say endpoint registered");
    let request = SayRequest {
        message: Some("hello".to_string()),
    };
    let error = grpc_nats_micro::client::request::<_, SayRequest, SayResponse>(
        &client,
        endpoint,
        ContentType::Protobuf,
        &request,
        STOPPED_SERVICE_TIMEOUT,
    )
    .await
    .expect_err("a stopped service must not respond");

    assert!(
        matches!(error, RequestError::Transport { .. }),
        "expected no responder, got {error:?}"
    );
}
