use buffa::{DecodeError, Enumeration, Message};
use serde_json::json;

use super::{assert_json_codec, assert_malformed, assert_proto_sequence, assert_wire_codec};
use crate::google::rpc::Code;
use crate::grpc_nats_micro::v1::{FailRequest, FailResponse, SayRequest, SayResponse};
use crate::nats::micro::v1alpha1::{ContentType, MethodOptions, ServiceOptions};

#[test]
fn echo_wire_distinguishes_absent_from_explicit_empty() {
    let absent = assert_json_codec::<SayRequest>(json!({}));
    let empty = assert_json_codec::<SayRequest>(json!({"message": ""}));
    let greeting = assert_json_codec::<SayRequest>(json!({"message": "hello"}));
    assert_eq!(absent.encode_to_vec(), b"");
    assert_eq!(empty.encode_to_vec(), b"\x0a\x00");
    assert_eq!(greeting.encode_to_vec(), b"\x0a\x05hello");
    assert_wire_codec(b"\x0a\x03old\x0a\x05hello", &greeting);
    assert_json_codec::<SayResponse>(json!({"message": "hello"}));
    assert_json_codec::<FailResponse>(json!({"message": "failed"}));
}

#[test]
fn status_code_json_preserves_known_names_and_unknown_numeric_values() {
    let known = assert_json_codec::<FailRequest>(json!({"code": "UNAVAILABLE", "message": "retry"}));
    assert_wire_codec(b"\x08\x0e\x12\x05retry", &known);
    assert_json_codec::<FailRequest>(json!({"code": 123, "message": "future code"}));
    assert_json_codec::<FailRequest>(json!({"code": "OK", "message": ""}));
    assert!(serde_json::from_value::<FailRequest>(json!({"code": "NOT_A_CODE"})).is_err());
    assert!(serde_json::from_value::<FailRequest>(json!({"code": 4294967296_u64})).is_err());
}

#[test]
fn discovery_options_preserve_metadata_and_content_type_policy() {
    for content_type in ["CONTENT_TYPE_PROTOBUF", "CONTENT_TYPE_JSON"] {
        assert_json_codec::<ServiceOptions>(json!({
            "version": "1.2.3", "description": "Schedule delivery",
            "metadata": {"region": "west", "owner": "scheduler"}, "contentType": content_type
        }));
    }
    let unrestricted = assert_json_codec::<ServiceOptions>(json!({"description": ""}));
    assert_eq!(unrestricted.encode_to_vec(), b"\x22\x00");
    let alias: ServiceOptions = serde_json::from_value(json!({
        "content_type": "CONTENT_TYPE_PROTOBUF"
    }))
    .expect("protobuf field alias");
    assert_eq!(
        serde_json::to_value(alias).expect("canonical field name"),
        json!({"contentType": "CONTENT_TYPE_PROTOBUF"})
    );

    let first = assert_json_codec::<MethodOptions>(json!({"metadata": {"region": "west", "owner": "scheduler"}}));
    let second = assert_json_codec::<MethodOptions>(json!({"metadata": {"region": "east"}}));
    let merged: MethodOptions = serde_json::from_value(json!({"metadata": {"region": "east", "owner": "scheduler"}}))
        .expect("merged endpoint metadata");
    assert_wire_codec(&[first.encode_to_vec(), second.encode_to_vec()].concat(), &merged);
}

#[test]
fn invalid_wire_tags_and_scalar_encodings_have_typed_failures() {
    assert_malformed::<SayRequest>(b"\x00", DecodeError::InvalidFieldNumber);
    assert_malformed::<SayRequest>(b"\x0f", DecodeError::InvalidWireType(7));
    assert_malformed::<SayRequest>(
        b"\x08\x01",
        DecodeError::WireTypeMismatch {
            field_number: 1,
            expected: 2,
            actual: 0,
        },
    );
    assert_malformed::<SayRequest>(b"\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<SayRequest>(b"\x0a\x02x", DecodeError::UnexpectedEof);
    assert_malformed::<FailRequest>(&[0x08, 0x80], DecodeError::UnexpectedEof);
    assert_malformed::<FailRequest>(
        &[0x08, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80],
        DecodeError::VarintTooLong,
    );
}

#[test]
fn explicit_rpc_defaults_are_present_even_when_built_without_json() {
    let request = SayRequest::default().with_message("");
    assert_wire_codec(b"\x0a\x00", &request);
    assert_wire_codec(b"\x0a\x00", &SayResponse::default().with_message(""));
    assert_wire_codec(b"\x0a\x00", &FailResponse::default().with_message(""));
    let failure = FailRequest::default().with_code(Code::OK).with_message("");
    assert_wire_codec(b"\x08\x00\x12\x00", &failure);
    assert_eq!(
        serde_json::to_value(failure).expect("explicit defaults"),
        json!({"code": "OK", "message": ""})
    );

    let options = ServiceOptions::default().with_version("").with_description("");
    assert_wire_codec(b"\x1a\x00\x22\x00", &options);
    assert_eq!(
        serde_json::to_value(options).expect("explicit discovery defaults"),
        json!({"version": "", "description": ""})
    );
}

#[test]
fn content_type_configuration_accepts_defined_names_and_numbers_only() {
    let expected = [
        (ContentType::Unspecified, 0, "CONTENT_TYPE_UNSPECIFIED"),
        (ContentType::Protobuf, 1, "CONTENT_TYPE_PROTOBUF"),
        (ContentType::Json, 2, "CONTENT_TYPE_JSON"),
    ];
    assert_eq!(ContentType::values(), expected.map(|(kind, _, _)| kind));
    assert_proto_sequence(
        expected.map(|(kind, _, _)| kind).to_vec(),
        json!(expected.map(|(_, _, name)| name)),
    );
    for (kind, number, name) in expected {
        assert_eq!(ContentType::from_i32(number), Some(kind));
        assert_eq!(kind.to_i32(), number);
        assert_eq!(kind.proto_name(), name);
        assert_eq!(
            serde_json::from_value::<ContentType>(json!(number)).expect("numeric content type"),
            kind
        );
        assert_eq!(
            serde_json::from_value::<ContentType>(json!(name)).expect("named content type"),
            kind
        );
        assert_eq!(serde_json::to_value(kind).expect("canonical content type"), json!(name));
    }
    assert_eq!(
        serde_json::from_value::<ContentType>(json!(null)).expect("unspecified content type"),
        ContentType::Unspecified
    );
    for invalid in [
        json!(-1),
        json!(3),
        json!(i64::MIN),
        json!(u64::MAX),
        json!("FUTURE"),
        json!({}),
        json!(true),
    ] {
        assert!(serde_json::from_value::<ContentType>(invalid).is_err());
    }
}
