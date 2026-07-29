mod prop_tests;

use super::*;
use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::direction::Direction;
use crate::error::CodecError;
use crate::id::encode_id_literal;
use crate::{
    decode, decode_canonical, decode_canonical_value, encode, encode_canonical, encode_canonical_value, from_json_value,
};
use async_nats::header::HeaderMap;

#[test]
fn from_json_value_rejects_mismatched_version() {
    let v3 = serde_json::json!({ "jsonrpc": "3.0", "id": 1, "method": "ping", "params": {} });
    assert!(matches!(
        from_json_value(&v3),
        Err(CodecError::UnsupportedVersion { found }) if found.as_deref() == Some("3.0")
    ));
}

#[test]
fn from_json_value_rejects_missing_version() {
    let missing = serde_json::json!({ "id": 1, "method": "ping", "params": {} });
    assert!(matches!(
        from_json_value(&missing),
        Err(CodecError::UnsupportedVersion { found: None })
    ));
}

#[test]
fn numeric_and_string_ids_are_distinct_on_the_wire() {
    let numeric = Message::Request {
        id: RequestId::Number(1),
        method: "ping".to_string(),
        params: serde_json::json!({}),
    };
    let string = Message::Request {
        id: RequestId::String("1".to_string()),
        method: "ping".to_string(),
        params: serde_json::json!({}),
    };

    let numeric_wire = encode(&numeric).unwrap();
    let string_wire = encode(&string).unwrap();

    assert_eq!(numeric_wire.headers.get(HEADER_ID).unwrap().as_str(), "1");
    assert_eq!(string_wire.headers.get(HEADER_ID).unwrap().as_str(), "\"1\"");
}

#[test]
fn error_response_is_discriminated_by_error_code_header() {
    let message = Message::Error {
        id: ResponseId::Number(9),
        code: -32000,
        message: "auth failed".to_string(),
        data: Some(serde_json::json!({"reason": "expired"})),
    };

    let wire = encode(&message).unwrap();
    assert_eq!(wire.headers.get(HEADER_ERROR_CODE).unwrap().as_str(), "-32000");
    let body: serde_json::Value = serde_json::from_slice(&wire.body).unwrap();
    assert_eq!(body["message"], "auth failed");
    assert_eq!(body["data"]["reason"], "expired");

    let decoded = decode(Direction::Response, None, &wire.headers, &wire.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn absent_id_on_response_means_null() {
    let message = Message::Success {
        id: ResponseId::Null,
        result: serde_json::json!(true),
    };
    let wire = encode(&message).unwrap();
    assert!(wire.headers.get(HEADER_ID).is_none());

    let decoded = decode(Direction::Response, None, &wire.headers, &wire.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn absent_id_on_request_means_notification() {
    let message = Message::Notification {
        method: "notify".to_string(),
        params: serde_json::json!({"x": 1}),
    };
    let wire = encode(&message).unwrap();
    assert!(wire.headers.get(HEADER_ID).is_none());

    let decoded = decode(Direction::Request, Some("notify"), &wire.headers, &wire.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn ambiguous_response_without_result_or_error_code_is_rejected() {
    let headers = async_nats::HeaderMap::new();
    let err = decode(Direction::Response, None, &headers, &[]).unwrap_err();
    assert!(matches!(err, CodecError::AmbiguousResponse));
}

#[test]
fn round_trip_via_json_value() {
    let original = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "unicode-\u{1F600}",
        "method": "test",
        "params": {"n": 42}
    });
    let message = from_json_value(&original).unwrap();
    let roundtrip = to_json_value(&message);
    assert_eq!(roundtrip, original);
}

#[test]
fn decode_request_with_id_and_empty_body_has_null_params() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ID, encode_id_literal(&RequestId::Number(1)));
    let msg = decode(Direction::Request, Some("ping"), &headers, &[]).unwrap();
    assert!(matches!(msg, Message::Request { params, .. } if params.is_null()));
}

#[test]
fn decode_request_without_id_but_error_code_is_ambiguous() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "1");
    let err = decode(Direction::Request, Some("ping"), &headers, &[]).unwrap_err();
    assert!(matches!(err, CodecError::AmbiguousResponse));
}

#[test]
fn decode_request_without_id_and_empty_body_is_notification() {
    let headers = HeaderMap::new();
    let msg = decode(Direction::Request, Some("notify"), &headers, &[]).unwrap();
    assert!(matches!(msg, Message::Notification { params, .. } if params.is_null()));
}

#[test]
fn decode_response_error_with_empty_body_has_empty_message() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "42");
    let msg = decode(Direction::Response, None, &headers, &[]).unwrap();
    assert!(matches!(msg, Message::Error { message, data, .. } if message.is_empty() && data.is_none()));
}

#[test]
fn decode_response_error_body_must_be_object() {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ERROR_CODE, "42");
    let err = decode(Direction::Response, None, &headers, b"\"scalar\"").unwrap_err();
    assert!(matches!(err, CodecError::Deserialize(_)));
}

#[test]
fn from_json_value_rejects_request_with_null_id() {
    let value = serde_json::json!({ "jsonrpc": "2.0", "method": "ping", "id": null, "params": {} });
    assert!(matches!(from_json_value(&value), Err(CodecError::RequestWithoutId)));
}

#[test]
fn from_json_value_parses_request_with_id() {
    let value = serde_json::json!({ "jsonrpc": "2.0", "method": "ping", "id": 7, "params": { "a": 1 } });
    assert!(matches!(from_json_value(&value).unwrap(), Message::Request { method, .. } if method == "ping"));
}

#[test]
fn from_json_value_rejects_error_without_code() {
    let value = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "error": { "message": "boom" } });
    assert!(matches!(from_json_value(&value), Err(CodecError::Deserialize(_))));
}

#[test]
fn canonical_request_uses_complete_body_without_jsonrpc_headers() {
    let message = Message::Request {
        id: RequestId::String("request-1".to_string()),
        method: "tools/list".to_string(),
        params: serde_json::json!({"cursor": "next"}),
    };

    let wire = encode_canonical(&message).unwrap();

    assert!(wire.headers.get(HEADER_ID).is_none());
    assert!(wire.headers.get(HEADER_ERROR_CODE).is_none());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "request-1",
            "method": "tools/list",
            "params": {"cursor": "next"}
        })
    );
    assert_eq!(
        decode_canonical(Direction::Request, Some("tools/list"), &wire.headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_notification_uses_complete_body() {
    let message = Message::Notification {
        method: "notifications/progress".to_string(),
        params: serde_json::json!({"progressToken": 7, "progress": 1}),
    };

    let wire = encode_canonical(&message).unwrap();

    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {"progressToken": 7, "progress": 1}
        })
    );
    assert_eq!(
        decode_canonical(
            Direction::Request,
            Some("notifications/progress"),
            &wire.headers,
            &wire.body,
        )
        .unwrap(),
        message
    );
}

#[test]
fn canonical_success_uses_complete_body_without_jsonrpc_headers() {
    let message = Message::Success {
        id: ResponseId::Number(9),
        result: serde_json::json!({"resultType": "complete"}),
    };

    let wire = encode_canonical(&message).unwrap();

    assert!(wire.headers.get(HEADER_ID).is_none());
    assert!(wire.headers.get(HEADER_ERROR_CODE).is_none());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "result": {"resultType": "complete"}
        })
    );
    assert_eq!(
        decode_canonical(Direction::Response, None, &wire.headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_error_uses_complete_body_without_jsonrpc_headers() {
    let message = Message::Error {
        id: ResponseId::Number(9),
        code: -32602,
        message: "invalid params".to_string(),
        data: Some(serde_json::json!({"field": "name"})),
    };

    let wire = encode_canonical(&message).unwrap();

    assert!(wire.headers.get(HEADER_ID).is_none());
    assert!(wire.headers.get(HEADER_ERROR_CODE).is_none());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 9,
            "error": {
                "code": -32602,
                "message": "invalid params",
                "data": {"field": "name"}
            }
        })
    );
    assert_eq!(
        decode_canonical(Direction::Response, None, &wire.headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_decode_rejects_method_projection_mismatch() {
    let message = Message::Request {
        id: RequestId::Number(1),
        method: "tools/list".to_string(),
        params: serde_json::json!({}),
    };
    let wire = encode_canonical(&message).unwrap();

    assert!(matches!(
        decode_canonical(Direction::Request, Some("resources/list"), &wire.headers, &wire.body),
        Err(CodecError::MethodProjectionMismatch { projected, actual })
            if projected == "resources/list" && actual == "tools/list"
    ));
}

#[test]
fn canonical_value_codec_preserves_absent_params_and_extension_members() {
    let value = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "x-vendor": {"trace": true}
    });

    let wire = encode_canonical_value(&value).unwrap();

    assert_eq!(serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(), value);
    assert_eq!(
        decode_canonical_value(Direction::Request, Some("ping"), &wire.headers, &wire.body).unwrap(),
        value
    );
}

#[test]
fn canonical_value_codec_rejects_invalid_jsonrpc_envelopes() {
    let cases = [
        (
            "scalar params",
            Direction::Request,
            Some("ping"),
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": null}),
        ),
        (
            "non-string method",
            Direction::Request,
            Some("ping"),
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": 7}),
        ),
        (
            "notification with result",
            Direction::Request,
            Some("notify"),
            serde_json::json!({"jsonrpc": "2.0", "method": "notify", "result": {}}),
        ),
        (
            "request with error",
            Direction::Request,
            Some("ping"),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "ping",
                "error": {"code": -32603, "message": "boom"}
            }),
        ),
        (
            "response with params",
            Direction::Response,
            None,
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "params": {}, "result": {}}),
        ),
        (
            "response without id",
            Direction::Response,
            None,
            serde_json::json!({"jsonrpc": "2.0", "result": {}}),
        ),
        (
            "success response with null id",
            Direction::Response,
            None,
            serde_json::json!({"jsonrpc": "2.0", "id": null, "result": {}}),
        ),
        (
            "response with result and error",
            Direction::Response,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {},
                "error": {"code": -32603, "message": "boom"}
            }),
        ),
        (
            "response without result or error",
            Direction::Response,
            None,
            serde_json::json!({"jsonrpc": "2.0", "id": 1}),
        ),
        (
            "non-object error",
            Direction::Response,
            None,
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": "boom"}),
        ),
        (
            "out-of-range error code",
            Direction::Response,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": 2147483648_i64, "message": "boom"}
            }),
        ),
        (
            "non-integer error code",
            Direction::Response,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": 1.5, "message": "boom"}
            }),
        ),
        (
            "non-string error message",
            Direction::Response,
            None,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32603, "message": 7}
            }),
        ),
    ];

    for (name, direction, method, value) in cases {
        assert!(encode_canonical_value(&value).is_err(), "encode accepted {name}");
        let body = serde_json::to_vec(&value).unwrap();
        assert!(
            decode_canonical_value(direction, method, &HeaderMap::new(), &body).is_err(),
            "decode accepted {name}"
        );
    }
}

#[test]
fn canonical_decode_accepts_matching_optional_legacy_projections() {
    let message = Message::Error {
        id: ResponseId::String("request-2".to_string()),
        code: -32603,
        message: "internal".to_string(),
        data: None,
    };
    let wire = encode_canonical(&message).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_ID, "\"request-2\"");
    headers.insert(HEADER_ERROR_CODE, "-32603");

    assert_eq!(
        decode_canonical(Direction::Response, None, &headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_decode_rejects_mismatched_optional_legacy_projections() {
    let message = Message::Error {
        id: ResponseId::Number(3),
        code: -32603,
        message: "internal".to_string(),
        data: None,
    };
    let wire = encode_canonical(&message).unwrap();
    let mut id_headers = HeaderMap::new();
    id_headers.insert(HEADER_ID, "4");
    assert!(matches!(
        decode_canonical(Direction::Response, None, &id_headers, &wire.body),
        Err(CodecError::IdProjectionMismatch { .. })
    ));

    let mut code_headers = HeaderMap::new();
    code_headers.insert(HEADER_ERROR_CODE, "-32602");
    assert!(matches!(
        decode_canonical(Direction::Response, None, &code_headers, &wire.body),
        Err(CodecError::ErrorCodeProjectionMismatch {
            projected: -32602,
            actual: -32603
        })
    ));
}
