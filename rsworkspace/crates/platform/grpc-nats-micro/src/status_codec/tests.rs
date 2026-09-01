use async_nats::HeaderMap;
use buffa::Enumeration as _;
use bytes::Bytes;
use trogonai_proto::google::rpc::{Code, Status};
use trogonai_proto::grpc_nats_micro::v1::SayResponse;

use super::{Outcome, ReplyError, decode_reply, encode_reply};
use crate::constants::{HEADER_CONTENT_TYPE, HEADER_ERROR, HEADER_ERROR_CODE};
use crate::content_type::ContentType;
use crate::service_fault::ServiceFault;

fn status(code: Code, message: &str) -> Status {
    Status {
        code: code.to_i32(),
        message: message.to_string(),
        details: Vec::new(),
    }
}

#[test]
fn a_success_reply_carries_no_error_headers() {
    let encoded =
        encode_reply(Outcome::Success(Bytes::from_static(b"body")), ContentType::Protobuf).expect("a success reply");

    assert!(encoded.headers.get(HEADER_ERROR_CODE).is_none());
    assert!(encoded.headers.get(HEADER_ERROR).is_none());
    assert_eq!(encoded.body, Bytes::from_static(b"body"));
}

#[test]
fn an_error_reply_carries_the_code_and_message_headers() {
    let encoded =
        encode_reply(Outcome::Error(ServiceFault::internal("boom")), ContentType::Json).expect("an error reply");

    assert_eq!(
        encoded
            .headers
            .get(HEADER_ERROR_CODE)
            .expect("error code header")
            .as_str(),
        Code::INTERNAL.to_i32().to_string()
    );
    assert_eq!(
        encoded
            .headers
            .get(HEADER_ERROR)
            .expect("error message header")
            .as_str(),
        "boom"
    );
}

#[test]
fn a_reply_without_the_error_header_decodes_as_the_response() {
    let response = SayResponse {
        message: Some("hello".to_string()),
    };
    let body = ContentType::Protobuf.encode(&response).expect("encode SayResponse");

    let decoded: SayResponse = decode_reply(None, &body, ContentType::Protobuf).expect("a success reply decodes");

    assert_eq!(decoded.message, Some("hello".to_string()));
}

/// ADR 0016 §4 makes the reply's own `Content-Type` authoritative, which is
/// what lets a rejection of the requested encoding still be readable.
#[test]
fn the_replys_own_content_type_overrides_the_requested_one() {
    let response = SayResponse {
        message: Some("hello".to_string()),
    };
    let body = ContentType::Json.encode(&response).expect("encode SayResponse");
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_CONTENT_TYPE, ContentType::Json.header_value());

    let decoded: SayResponse =
        decode_reply(Some(&headers), &body, ContentType::Protobuf).expect("the reply names its own encoding");

    assert_eq!(decoded.message, Some("hello".to_string()));
}

#[test]
fn an_error_code_header_that_is_not_a_code_is_reported() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "RESOURCE_EXHAUSTED");

    let error =
        decode_reply::<SayResponse>(Some(&headers), b"", ContentType::Protobuf).expect_err("the header is not a code");

    let ReplyError::ErrorCode(cause) = error else {
        panic!("expected an error code failure");
    };
    assert_eq!(
        cause.to_string(),
        "service error code RESOURCE_EXHAUSTED is not an integer"
    );
}

#[test]
fn an_error_reply_surfaces_the_whole_status() {
    let body = status(Code::NOT_FOUND, "missing");
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, Code::NOT_FOUND.to_i32().to_string().as_str());
    let payload = ContentType::Protobuf.encode(&body).expect("encode Status");

    let error =
        decode_reply::<SayResponse>(Some(&headers), &payload, ContentType::Protobuf).expect_err("an error reply");

    let ReplyError::Service(service_error) = error else {
        panic!("expected a micro service error");
    };
    assert_eq!(service_error.code().code(), Code::NOT_FOUND);
    assert_eq!(service_error.message(), "missing");
    assert_eq!(service_error.status(), &body);
    assert_eq!(
        service_error.to_string(),
        "nats micro service error (NOT_FOUND): missing"
    );
    assert_eq!(service_error.into_status(), body);
}

#[test]
fn an_undecodable_error_body_is_reported() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, Code::NOT_FOUND.to_i32().to_string().as_str());

    let error = decode_reply::<SayResponse>(Some(&headers), b"not json", ContentType::Json)
        .expect_err("the body is not a Status");

    assert!(matches!(error, ReplyError::Decode(_)));
}
