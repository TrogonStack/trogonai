mod prop_tests;

use super::*;
use crate::constants::{HEADER_ERROR_CODE, HEADER_ID};
use crate::direction::Direction;
use crate::error::CodecError;
use crate::{decode, decode_value, encode, encode_value, from_json_value};
use async_nats::header::HeaderMap;

#[test]
fn typed_error_conversion_preserves_escaped_identity_and_header_projections() {
    let message = Message::Error {
        id: ResponseId::String("caller\n\"reply\"".into()),
        code: -32118,
        message: "verification rejected\nrequest".into(),
        data: Some(serde_json::json!({"rule": "auth", "details": [null, true, 7]})),
    };
    let encoded = Encoded::from(&message);
    assert_eq!(
        encoded.headers.get(HEADER_ID).unwrap().as_str(),
        "\"caller\\n\\\"reply\\\"\""
    );
    assert_eq!(encoded.headers.get(HEADER_ERROR_CODE).unwrap().as_str(), "-32118");
    assert_eq!(
        decode(Direction::Response, None, &encoded.headers, &encoded.body).unwrap(),
        message
    );
}

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
fn canonical_request_without_params_round_trips() {
    let message = Message::Request {
        id: RequestId::Number(1),
        method: "ping".to_string(),
        params: serde_json::Value::Null,
    };
    let encoded = encode(&message).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&encoded.body).unwrap();
    assert!(body.get("params").is_none());

    let decoded = decode(Direction::Request, Some("ping"), &encoded.headers, &encoded.body).unwrap();
    assert_eq!(decoded, message);
}

#[test]
fn canonical_notification_without_params_round_trips() {
    let message = Message::Notification {
        method: "notifications/initialized".to_string(),
        params: serde_json::Value::Null,
    };
    let encoded = encode(&message).unwrap();
    let body: serde_json::Value = serde_json::from_slice(&encoded.body).unwrap();
    assert!(body.get("params").is_none());

    let decoded = decode(
        Direction::Request,
        Some("notifications/initialized"),
        &encoded.headers,
        &encoded.body,
    )
    .unwrap();
    assert_eq!(decoded, message);
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
fn canonical_request_uses_complete_body_with_derived_jsonrpc_headers() {
    let message = Message::Request {
        id: RequestId::String("request-1".to_string()),
        method: "tools/list".to_string(),
        params: serde_json::json!({"cursor": "next"}),
    };

    let wire = encode(&message).unwrap();

    assert_eq!(wire.headers.get(HEADER_ID).unwrap().as_str(), "\"request-1\"");
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
        decode(Direction::Request, Some("tools/list"), &wire.headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_notification_uses_complete_body_without_jsonrpc_id() {
    let message = Message::Notification {
        method: "notifications/progress".to_string(),
        params: serde_json::json!({"progressToken": 7, "progress": 1}),
    };

    let wire = encode(&message).unwrap();

    assert!(wire.headers.get(HEADER_ID).is_none());
    assert!(wire.headers.get(HEADER_ERROR_CODE).is_none());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/progress",
            "params": {"progressToken": 7, "progress": 1}
        })
    );
    assert_eq!(
        decode(
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
fn canonical_success_uses_complete_body_with_derived_jsonrpc_headers() {
    let message = Message::Success {
        id: ResponseId::Number(9),
        result: serde_json::json!({"resultType": "complete"}),
    };

    let wire = encode(&message).unwrap();

    assert_eq!(wire.headers.get(HEADER_ID).unwrap().as_str(), "9");
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
        decode(Direction::Response, None, &wire.headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_error_uses_complete_body_with_derived_jsonrpc_headers() {
    let message = Message::Error {
        id: ResponseId::Number(9),
        code: -32602,
        message: "invalid params".to_string(),
        data: Some(serde_json::json!({"field": "name"})),
    };

    let wire = encode(&message).unwrap();

    assert_eq!(wire.headers.get(HEADER_ID).unwrap().as_str(), "9");
    assert_eq!(wire.headers.get(HEADER_ERROR_CODE).unwrap().as_str(), "-32602");
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
        decode(Direction::Response, None, &wire.headers, &wire.body).unwrap(),
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
    let wire = encode(&message).unwrap();

    assert!(matches!(
        decode(Direction::Request, Some("resources/list"), &wire.headers, &wire.body),
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

    let wire = encode_value(&value).unwrap();

    assert_eq!(serde_json::from_slice::<serde_json::Value>(&wire.body).unwrap(), value);
    assert_eq!(
        decode_value(Direction::Request, Some("ping"), &wire.headers, &wire.body).unwrap(),
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
        assert!(encode_value(&value).is_err(), "encode accepted {name}");
        let body = serde_json::to_vec(&value).unwrap();
        assert!(
            decode_value(direction, method, &HeaderMap::new(), &body).is_err(),
            "decode accepted {name}"
        );
    }
}

#[test]
fn canonical_decode_accepts_matching_derived_projections() {
    let message = Message::Error {
        id: ResponseId::String("request-2".to_string()),
        code: -32603,
        message: "internal".to_string(),
        data: None,
    };
    let wire = encode(&message).unwrap();

    assert_eq!(wire.headers.get(HEADER_ID).unwrap().as_str(), "\"request-2\"");
    assert_eq!(wire.headers.get(HEADER_ERROR_CODE).unwrap().as_str(), "-32603");
    assert_eq!(
        decode(Direction::Response, None, &wire.headers, &wire.body).unwrap(),
        message
    );
}

#[test]
fn canonical_decode_rejects_mismatched_derived_projections() {
    let message = Message::Error {
        id: ResponseId::Number(3),
        code: -32603,
        message: "internal".to_string(),
        data: None,
    };
    let wire = encode(&message).unwrap();
    let mut id_headers = HeaderMap::new();
    id_headers.insert(HEADER_ID, "4");
    assert!(matches!(
        decode(Direction::Response, None, &id_headers, &wire.body),
        Err(CodecError::IdProjectionMismatch { .. })
    ));

    let mut code_headers = HeaderMap::new();
    code_headers.insert(HEADER_ERROR_CODE, "-32602");
    assert!(matches!(
        decode(Direction::Response, None, &code_headers, &wire.body),
        Err(CodecError::ErrorCodeProjectionMismatch {
            projected: -32602,
            actual: -32603
        })
    ));
}

#[test]
fn response_id_from_request_value_keeps_canonical_ids() {
    assert_eq!(
        ResponseId::from_request_value(&serde_json::json!("my-string-id")),
        ResponseId::String("my-string-id".to_string())
    );
    assert_eq!(
        ResponseId::from_request_value(&serde_json::json!(7)),
        ResponseId::Number(7)
    );
    assert_eq!(
        ResponseId::from_request_value(&serde_json::Value::Null),
        ResponseId::Null
    );
}

#[test]
fn response_id_from_request_value_nulls_non_canonical_ids() {
    for value in [
        serde_json::json!(1.5),
        serde_json::json!({ "nested": true }),
        serde_json::json!([1]),
        serde_json::json!(true),
    ] {
        assert_eq!(ResponseId::from_request_value(&value), ResponseId::Null);
    }
}

#[test]
fn responses_built_from_coerced_ids_stay_decodable() {
    // The property the coercion exists for: whatever a caller sends as `id`, the
    // envelope an edge answers with still parses as canonical JSON-RPC.
    for value in [
        serde_json::json!("abc"),
        serde_json::json!(1),
        serde_json::json!(1.5),
        serde_json::json!({ "nested": true }),
    ] {
        let envelope = to_json_value(&Message::Error {
            id: ResponseId::from_request_value(&value),
            code: crate::INVALID_REQUEST,
            message: "invalid request".to_string(),
            data: None,
        });
        assert!(from_json_value(&envelope).is_ok(), "id {value} produced {envelope}");
    }
}
