use std::time::Duration;

use async_nats::HeaderMap;
use buffa::Enumeration as _;
use bytes::Bytes;
use trogon_nats::AdvancedMockNatsClient;
use trogonai_proto::google::rpc::{Code, Status};
use trogonai_proto::grpc_nats_micro::v1::{SayRequest, SayResponse};

use super::{RequestError, request};
use crate::binding::{EndpointBinding, ServiceBinding};
use crate::constants::HEADER_ERROR_CODE;
use crate::content_type::ContentType;
use crate::method_name::MethodName;
use crate::service_name::ServiceName;
use crate::subject_prefix::SubjectPrefix;

const SAY_SUBJECT: &str = "echo.v1.EchoService.Say";
const REQUEST_TIMEOUT: Duration = Duration::from_millis(50);

fn binding() -> ServiceBinding {
    ServiceBinding::new(
        ServiceName::new("EchoService").expect("valid service name"),
        "0.1.0",
        SubjectPrefix::new("echo.v1").expect("valid subject prefix"),
    )
    .with_method(MethodName::new("Say").expect("valid method name"))
    .expect("derive the Say subject")
}

fn say_endpoint(binding: &ServiceBinding) -> &EndpointBinding {
    binding.endpoints().first().expect("the Say endpoint is registered")
}

fn say(message: &str) -> SayRequest {
    SayRequest {
        message: Some(message.to_string()),
    }
}

#[tokio::test]
async fn decodes_a_successful_reply() {
    let client = AdvancedMockNatsClient::new();
    let reply = SayResponse {
        message: Some("hello".to_string()),
    };
    let body = ContentType::Protobuf.encode(&reply).expect("encode SayResponse");
    client.set_response_wire(SAY_SUBJECT, HeaderMap::new(), Bytes::from(body));
    let binding = binding();

    let response: SayResponse = request(
        &client,
        say_endpoint(&binding),
        ContentType::Protobuf,
        &say("hello"),
        REQUEST_TIMEOUT,
    )
    .await
    .expect("the reply decodes");

    assert_eq!(response.message, Some("hello".to_string()));
}

#[tokio::test]
async fn surfaces_a_service_error_reply() {
    let client = AdvancedMockNatsClient::new();
    let status = Status {
        code: Code::NOT_FOUND.to_i32(),
        message: "missing".to_string(),
        details: Vec::new(),
    };
    let body = ContentType::Protobuf.encode(&status).expect("encode Status");
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, Code::NOT_FOUND.to_i32().to_string().as_str());
    client.set_response_wire(SAY_SUBJECT, headers, Bytes::from(body));
    let binding = binding();

    let error = request::<_, SayRequest, SayResponse>(
        &client,
        say_endpoint(&binding),
        ContentType::Protobuf,
        &say("hello"),
        REQUEST_TIMEOUT,
    )
    .await
    .expect_err("the reply is a service error");

    assert!(matches!(error, RequestError::Reply(_)), "{error:?}");
}

/// The transport's own failure is kept rather than rendered, so a caller can
/// match on `no responders` or a lost connection instead of parsing a message.
#[tokio::test]
async fn keeps_the_transports_own_failure() {
    let client = AdvancedMockNatsClient::new();
    client.fail_next_request();
    let binding = binding();

    let error = request::<_, SayRequest, SayResponse>(
        &client,
        say_endpoint(&binding),
        ContentType::Protobuf,
        &say("hello"),
        REQUEST_TIMEOUT,
    )
    .await
    .expect_err("the transport failed");

    assert!(
        matches!(&error, RequestError::Transport { subject, .. } if subject == SAY_SUBJECT),
        "{error:?}"
    );
}

#[tokio::test]
async fn reports_a_round_trip_that_outlives_its_deadline() {
    let client = AdvancedMockNatsClient::new();
    client.hang_next_request();
    let binding = binding();

    let error = request::<_, SayRequest, SayResponse>(
        &client,
        say_endpoint(&binding),
        ContentType::Protobuf,
        &say("hello"),
        REQUEST_TIMEOUT,
    )
    .await
    .expect_err("the round trip outlived its deadline");

    assert!(
        matches!(&error, RequestError::Timeout { subject } if subject == SAY_SUBJECT),
        "{error:?}"
    );
}
