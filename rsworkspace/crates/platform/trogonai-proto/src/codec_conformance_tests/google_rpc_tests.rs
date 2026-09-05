use buffa::{DecodeError, Message};
use buffa_types::google::protobuf::Any;
use serde_json::json;

use super::{assert_json_codec, assert_malformed, assert_wire_codec};
use crate::google::rpc::{
    BadRequest, DebugInfo, ErrorInfo, Help, LocalizedMessage, PreconditionFailure, QuotaFailure, RequestInfo,
    ResourceInfo, RetryInfo, Status,
};

#[test]
fn structured_error_details_preserve_wire_and_json_contracts() {
    assert_json_codec::<ErrorInfo>(json!({
        "reason": "QUOTA_EXCEEDED", "domain": "scheduler.example",
        "metadata": {"limit": "100", "region": "west"}
    }));
    assert_json_codec::<RetryInfo>(json!({"retryDelay": "1.250s"}));
    assert_json_codec::<DebugInfo>(json!({
        "stackEntries": ["schedule", "deliver"], "detail": "delivery timeout"
    }));
    assert_json_codec::<QuotaFailure>(json!({"violations": [{
        "subject": "projects/example", "description": "daily limit exceeded",
        "apiService": "scheduler.example", "quotaMetric": "deliveries",
        "quotaId": "daily-deliveries", "quotaDimensions": {"region": "west"},
        "quotaValue": "9007199254740993"
    }]}));
    assert_json_codec::<PreconditionFailure>(json!({"violations": [{
        "type": "VERSION", "subject": "schedules/backup", "description": "version changed"
    }]}));
    assert_json_codec::<BadRequest>(json!({"fieldViolations": [{
        "field": "schedule.cron.expr", "description": "invalid expression", "reason": "INVALID_CRON",
        "localizedMessage": {"locale": "es", "message": "Expresión inválida"}
    }]}));
    assert_json_codec::<RequestInfo>(json!({"requestId": "request-7", "servingData": "region-west"}));
    assert_json_codec::<ResourceInfo>(json!({
        "resourceType": "schedule", "resourceName": "schedules/backup",
        "owner": "projects/example", "description": "schedule was removed"
    }));
    assert_json_codec::<Help>(json!({"links": [{
        "description": "Troubleshooting", "url": "https://example.com/help"
    }]}));
    assert_json_codec::<LocalizedMessage>(json!({"locale": "ja", "message": "再試行してください"}));
}

#[test]
fn status_envelope_retains_typed_error_details() {
    let detail = assert_json_codec::<ErrorInfo>(json!({
        "reason": "SCHEDULE_MISSING", "domain": "scheduler.example", "metadata": {"scheduleId": "backup"}
    }));
    let retry = assert_json_codec::<RetryInfo>(json!({"retryDelay": "2s"}));
    let status = Status {
        code: 5,
        message: "schedule missing".to_owned(),
        details: vec![
            Any::pack(&detail, ErrorInfo::TYPE_URL),
            Any::pack(&retry, RetryInfo::TYPE_URL),
        ],
    };
    let wire = status.encode_to_vec();
    assert_wire_codec(&wire, &status);
    let decoded = Status::decode_from_slice(&wire).expect("status decode");
    assert_eq!(
        decoded.details[0]
            .unpack_if::<ErrorInfo>(ErrorInfo::TYPE_URL)
            .expect("typed detail"),
        Some(detail)
    );
    assert_eq!(
        decoded.details[1]
            .unpack_if::<RetryInfo>(RetryInfo::TYPE_URL)
            .expect("typed retry"),
        Some(retry)
    );
    assert_eq!(
        decoded.details[0]
            .unpack_if::<RetryInfo>(RetryInfo::TYPE_URL)
            .expect("different detail type"),
        None
    );
    assert_json_codec::<Status>(json!({"code": 5, "message": "schedule missing"}));
}

#[test]
fn duplicate_scalars_and_map_keys_use_last_value_while_repeated_fields_append() {
    let first = assert_json_codec::<ErrorInfo>(json!({
        "reason": "OLD", "domain": "scheduler.example", "metadata": {"region": "west", "limit": "10"}
    }));
    let second = assert_json_codec::<ErrorInfo>(json!({"reason": "NEW", "metadata": {"region": "east"}}));
    let expected: ErrorInfo = serde_json::from_value(json!({
        "reason": "NEW", "domain": "scheduler.example", "metadata": {"region": "east", "limit": "10"}
    }))
    .expect("merged error");
    assert_wire_codec(&[first.encode_to_vec(), second.encode_to_vec()].concat(), &expected);

    let first = assert_json_codec::<DebugInfo>(json!({"stackEntries": ["schedule"], "detail": "old"}));
    let second = assert_json_codec::<DebugInfo>(json!({"stackEntries": ["deliver"], "detail": "new"}));
    let expected: DebugInfo = serde_json::from_value(json!({"stackEntries": ["schedule", "deliver"], "detail": "new"}))
        .expect("merged stack");
    assert_wire_codec(&[first.encode_to_vec(), second.encode_to_vec()].concat(), &expected);
}

#[test]
fn malformed_map_entry_and_nested_error_detail_are_rejected_eagerly() {
    assert_malformed::<ErrorInfo>(b"\x1a\x03\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<ErrorInfo>(b"\x1a\x04\x0a\x01x", DecodeError::UnexpectedEof);
    assert_malformed::<BadRequest>(b"\x0a\x03\x0a\x01\xff", DecodeError::InvalidUtf8);
    assert_malformed::<Status>(b"\x1a\x03\x0a\x01\xff", DecodeError::InvalidUtf8);
}
