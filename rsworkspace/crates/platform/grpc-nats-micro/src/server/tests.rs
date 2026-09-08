use std::future::Future;
use std::pin::Pin;

use async_nats::{HeaderMap, Subject};
use buffa::Enumeration as _;

use trogon_nats::AdvancedMockNatsClient;
use trogonai_proto::google::rpc::Code;
use trogonai_proto::nats::micro::v1alpha1::{ContentType as ProtoContentType, ServiceOptions};

use super::{EndpointHandler, ReplyKind, reply_error, resolve, warn_if_undelivered, warn_unencodable};
use crate::constants::{HEADER_CONTENT_TYPE, HEADER_ERROR_CODE};
use crate::content_type::{ContentType, EncodeError};
use crate::service_fault::ServiceFault;

const REPLY_SUBJECT: &str = "_INBOX.reply";

struct EchoHandler;

impl EndpointHandler for EchoHandler {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        _content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move { Ok(request_bytes.to_vec()) })
    }
}

struct FailingHandler;

impl EndpointHandler for FailingHandler {
    fn handle<'a>(
        &'a self,
        _request_bytes: &'a [u8],
        _content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async { Err(ServiceFault::internal("boom")) })
    }
}

fn content_type_header(value: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, value);
    headers
}

fn protobuf_only() -> ServiceOptions {
    ServiceOptions {
        content_type: ProtoContentType::CONTENT_TYPE_PROTOBUF.into(),
        ..Default::default()
    }
}

fn reply_subject() -> Subject {
    Subject::from_static(REPLY_SUBJECT)
}

#[tokio::test]
async fn the_handler_runs_under_the_negotiated_encoding() {
    let headers = content_type_header(ContentType::Json.header_value());

    let (content_type, outcome) = resolve(Some(&headers), b"payload", &ServiceOptions::default(), &EchoHandler).await;

    assert_eq!(content_type, ContentType::Json);
    assert_eq!(outcome.expect("the handler succeeded"), b"payload".to_vec());
}

#[tokio::test]
async fn a_request_without_a_content_type_header_negotiates_the_policy_default() {
    let (content_type, outcome) = resolve(None, b"payload", &ServiceOptions::default(), &FailingHandler).await;

    assert_eq!(content_type, ContentType::Protobuf);
    assert_eq!(outcome.expect_err("the handler failed").code().code(), Code::INTERNAL);
}

#[tokio::test]
async fn a_rejected_encoding_is_reported_in_the_encoding_the_caller_asked_for() {
    let headers = content_type_header(ContentType::Json.header_value());

    let (content_type, outcome) = resolve(Some(&headers), b"payload", &protobuf_only(), &EchoHandler).await;

    assert_eq!(content_type, ContentType::Json);
    assert_eq!(
        outcome.expect_err("a json caller is turned away").code().code(),
        Code::INVALID_ARGUMENT
    );
}

#[tokio::test]
async fn an_encoding_the_binding_does_not_speak_falls_back_to_protobuf() {
    let headers = content_type_header("application/xml");

    let (content_type, outcome) = resolve(Some(&headers), b"payload", &ServiceOptions::default(), &EchoHandler).await;

    assert_eq!(content_type, ContentType::Protobuf);
    assert_eq!(
        outcome.expect_err("an unknown encoding is turned away").code().code(),
        Code::INVALID_ARGUMENT
    );
}

#[tokio::test]
async fn an_error_reply_is_published_on_the_reply_subject() {
    let client = AdvancedMockNatsClient::new();

    reply_error(
        &client,
        Some(reply_subject()),
        ServiceFault::internal("boom"),
        ContentType::Protobuf,
    )
    .await
    .expect("an encodable status");

    assert_eq!(client.published_messages(), vec![REPLY_SUBJECT.to_string()]);
    let headers = client.published_headers();
    let headers = headers.first().expect("the error reply carries headers");
    assert_eq!(
        headers.get(HEADER_ERROR_CODE).expect("error code header").as_str(),
        Code::INTERNAL.to_i32().to_string()
    );
    assert_eq!(
        headers.get(HEADER_CONTENT_TYPE).expect("content type header").as_str(),
        ContentType::Protobuf.header_value()
    );
}

#[tokio::test]
async fn a_request_without_a_reply_subject_drops_the_error_reply() {
    let client = AdvancedMockNatsClient::new();

    reply_error(&client, None, ServiceFault::internal("boom"), ContentType::Protobuf)
        .await
        .expect("an encodable status");

    assert!(client.published_messages().is_empty());
}

#[tokio::test]
async fn a_publish_failure_leaves_the_error_reply_undelivered() {
    let client = AdvancedMockNatsClient::new();
    client.fail_next_publish();

    reply_error(
        &client,
        Some(reply_subject()),
        ServiceFault::internal("boom"),
        ContentType::Protobuf,
    )
    .await
    .expect("an encodable status");

    assert!(client.published_messages().is_empty());
}

#[test]
fn an_undelivered_reply_is_reported_for_either_half_of_the_contract() {
    warn_if_undelivered(Err(std::io::Error::other("gone")), ReplyKind::Success);
    warn_if_undelivered(Err(std::io::Error::other("gone")), ReplyKind::Error);
    warn_if_undelivered(Ok::<(), std::io::Error>(()), ReplyKind::Success);
}

#[test]
fn a_status_that_will_not_encode_is_reported() {
    let source = serde_json::from_str::<u8>("not a number").expect_err("a malformed number");
    warn_unencodable(&EncodeError::Json(source));
}
