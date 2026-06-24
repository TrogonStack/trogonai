use proptest::prelude::*;

use crate::direction::Direction;
use crate::id::{RequestId, ResponseId};
use crate::message::Message;
use crate::{decode, encode, from_json_value, to_json_value};

fn arb_request_id() -> impl Strategy<Value = RequestId> {
    prop_oneof![
        any::<i64>().prop_map(RequestId::Number),
        r#".{0,32}"#.prop_map(RequestId::String),
    ]
}

fn arb_response_id() -> impl Strategy<Value = ResponseId> {
    prop_oneof![
        any::<i64>().prop_map(ResponseId::Number),
        r#".{0,32}"#.prop_map(ResponseId::String),
        Just(ResponseId::Null),
    ]
}

fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i64>().prop_map(|n| serde_json::Value::Number(n.into())),
        r#".{0,32}"#.prop_map(|s| serde_json::Value::String(s)),
        prop::collection::vec(any::<i64>(), 0..4).prop_map(|items| {
            serde_json::Value::Array(items.into_iter().map(|n| serde_json::Value::Number(n.into())).collect())
        }),
    ]
}

fn arb_message() -> impl Strategy<Value = Message> {
    prop_oneof![
        (arb_request_id(), r#"[a-z]{1,16}"#, arb_json_value()).prop_map(|(id, method, params)| Message::Request {
            id,
            method,
            params,
        }),
        (r#"[a-z]{1,16}"#, arb_json_value()).prop_map(|(method, params)| Message::Notification { method, params }),
        (arb_response_id(), arb_json_value()).prop_map(|(id, result)| Message::Success { id, result }),
        (
            arb_response_id(),
            any::<i32>(),
            r#".{0,64}"#,
            prop::option::of(arb_json_value())
        )
            .prop_map(|(id, code, message, data)| Message::Error {
                id,
                code,
                message,
                data,
            }),
    ]
}

proptest! {
    #[test]
    fn decode_encode_roundtrip_preserves_message(message in arb_message()) {
        let wire = encode(&message).unwrap();
        let direction = match message {
            Message::Request { .. } | Message::Notification { .. } => Direction::Request,
            Message::Success { .. } | Message::Error { .. } => Direction::Response,
        };
        let method = message.method();
        let decoded = decode(direction, method, &wire.headers, &wire.body).unwrap();
        prop_assert_eq!(decoded, message);
    }

    #[test]
    fn json_value_roundtrip_preserves_canonical_message(message in arb_message()) {
        let json = to_json_value(&message);
        let parsed = from_json_value(&json).unwrap();
        prop_assert_eq!(&parsed, &message);
        prop_assert_eq!(to_json_value(&parsed), json);
    }

    #[test]
    fn numeric_one_and_string_one_stay_distinct(id in prop_oneof![Just(RequestId::Number(1)), Just(RequestId::String("1".to_string()))]) {
        let message = Message::Request {
            id,
            method: "m".to_string(),
            params: serde_json::json!({}),
        };
        let wire = encode(&message).unwrap();
        let header = wire.headers.get(crate::HEADER_ID).unwrap().as_str();
        match message {
            Message::Request { id: RequestId::Number(1), .. } => prop_assert_eq!(header, "1"),
            Message::Request { id: RequestId::String(_), .. } => prop_assert_eq!(header, "\"1\""),
            _ => unreachable!(),
        }
    }

    #[test]
    fn error_iff_error_code_header_present(message in arb_message()) {
        let wire = encode(&message).unwrap();
        let has_error_code = wire.headers.get(crate::HEADER_ERROR_CODE).is_some();
        prop_assert_eq!(has_error_code, message.is_error());
    }
}
