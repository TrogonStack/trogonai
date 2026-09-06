use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, Enumeration, Message, OwnedView};
use serde_json::json;

use super::{assert_json_codec, assert_proto_sequence};
use crate::google::rpc::*;

use super::retained_fixture::retained_detail;

#[test]
fn retained_error_information_keeps_metadata_and_binary_identity_after_transfer() {
    retained_detail!(
        ErrorInfo,
        ErrorInfoOwnedView,
        ErrorInfoView<'static>,
        json!({
            "reason": "QUOTA_EXCEEDED", "domain": "scheduler.example", "metadata": {"region": "west"}
        }),
        |handle| {
            assert_eq!(handle.reason(), "QUOTA_EXCEEDED");
            assert_eq!(handle.domain(), "scheduler.example");
            assert_eq!(handle.metadata().get("region"), Some(&"west"));
        }
    );
    retained_detail!(
        RequestInfo,
        RequestInfoOwnedView,
        RequestInfoView<'static>,
        json!({
            "requestId": "request-7", "servingData": "region-west"
        }),
        |handle| {
            assert_eq!(handle.request_id(), "request-7");
            assert_eq!(handle.serving_data(), "region-west");
        }
    );
    retained_detail!(
        ResourceInfo,
        ResourceInfoOwnedView,
        ResourceInfoView<'static>,
        json!({
            "resourceType": "schedule", "resourceName": "schedules/backup", "owner": "projects/example", "description": "missing"
        }),
        |handle| {
            assert_eq!(handle.resource_type(), "schedule");
            assert_eq!(handle.resource_name(), "schedules/backup");
            assert_eq!(handle.owner(), "projects/example");
            assert_eq!(handle.description(), "missing");
        }
    );
}

#[test]
fn retained_nested_diagnostics_outlive_source_containers() {
    retained_detail!(
        DebugInfo,
        DebugInfoOwnedView,
        DebugInfoView<'static>,
        json!({
            "stackEntries": ["schedule", "deliver"], "detail": "timeout"
        }),
        |handle| {
            assert_eq!(&**handle.stack_entries(), &["schedule", "deliver"]);
            assert_eq!(handle.detail(), "timeout");
        }
    );
    retained_detail!(
        RetryInfo,
        RetryInfoOwnedView,
        RetryInfoView<'static>,
        json!({"retryDelay": "1.250s"}),
        |handle| {
            assert!(handle.retry_delay().is_set());
            assert_eq!(handle.retry_delay().seconds, 1);
            assert_eq!(handle.retry_delay().nanos, 250_000_000);
        }
    );
    retained_detail!(
        LocalizedMessage,
        LocalizedMessageOwnedView,
        LocalizedMessageView<'static>,
        json!({
            "locale": "es", "message": "Inténtalo de nuevo"
        }),
        |handle| {
            assert_eq!(handle.locale(), "es");
            assert_eq!(handle.message(), "Inténtalo de nuevo");
        }
    );
    retained_detail!(
        Help,
        HelpOwnedView,
        HelpView<'static>,
        json!({"links": [{
            "description": "Retry guide", "url": "https://example.com/retry"
        }]}),
        |handle| {
            assert_eq!(handle.links().len(), 1);
            assert_eq!(handle.links()[0].description, "Retry guide");
            assert_eq!(handle.links()[0].url, "https://example.com/retry");
        }
    );
    retained_detail!(
        help::Link,
        help::LinkOwnedView,
        help::LinkView<'static>,
        json!({
            "description": "Retry guide", "url": "https://example.com/retry"
        }),
        |handle| {
            assert_eq!(handle.description(), "Retry guide");
            assert_eq!(handle.url(), "https://example.com/retry");
        }
    );
}

#[test]
fn retained_quota_and_precondition_violations_preserve_nested_coordinates() {
    let quota = json!({
        "subject": "projects/example", "description": "limit exceeded", "apiService": "scheduler.example",
        "quotaMetric": "deliveries", "quotaId": "daily", "quotaDimensions": {"region": "west"},
        "quotaValue": "9007199254740993", "futureQuotaValue": "9007199254740994"
    });
    retained_detail!(
        QuotaFailure,
        QuotaFailureOwnedView,
        QuotaFailureView<'static>,
        json!({"violations": [quota.clone()]}),
        |handle| {
            assert_eq!(handle.violations().len(), 1);
            assert_eq!(handle.violations()[0].subject, "projects/example");
        }
    );
    retained_detail!(
        quota_failure::Violation,
        quota_failure::ViolationOwnedView,
        quota_failure::ViolationView<'static>,
        quota,
        |handle| {
            assert_eq!(handle.subject(), "projects/example");
            assert_eq!(handle.description(), "limit exceeded");
            assert_eq!(handle.api_service(), "scheduler.example");
            assert_eq!(handle.quota_metric(), "deliveries");
            assert_eq!(handle.quota_id(), "daily");
            assert_eq!(handle.quota_dimensions().get("region"), Some(&"west"));
            assert_eq!(handle.quota_value(), 9_007_199_254_740_993);
            assert_eq!(handle.future_quota_value(), Some(9_007_199_254_740_994));
        }
    );
    let violation = json!({"type": "VERSION", "subject": "schedules/backup", "description": "revision mismatch"});
    retained_detail!(
        PreconditionFailure,
        PreconditionFailureOwnedView,
        PreconditionFailureView<'static>,
        json!({"violations": [violation.clone()]}),
        |handle| {
            assert_eq!(handle.violations().len(), 1);
            assert_eq!(handle.violations()[0].subject, "schedules/backup");
        }
    );
    retained_detail!(
        precondition_failure::Violation,
        precondition_failure::ViolationOwnedView,
        precondition_failure::ViolationView<'static>,
        violation,
        |handle| {
            assert_eq!(handle.r#type(), "VERSION");
            assert_eq!(handle.subject(), "schedules/backup");
            assert_eq!(handle.description(), "revision mismatch");
        }
    );
}

#[test]
fn quota_rollout_preserves_an_explicit_zero_limit() {
    let no_rollout = assert_json_codec::<quota_failure::Violation>(json!({}));
    let zero_limit = assert_json_codec::<quota_failure::Violation>(json!({"futureQuotaValue": "0"}));
    assert_eq!(
        zero_limit,
        quota_failure::Violation::default().with_future_quota_value(0)
    );
    assert_eq!(no_rollout.encode_to_vec(), b"");
    assert_eq!(zero_limit.encode_to_vec(), b"\x40\x00");
    retained_detail!(
        quota_failure::Violation,
        quota_failure::ViolationOwnedView,
        quota_failure::ViolationView<'static>,
        json!({"futureQuotaValue": "0"}),
        |handle| {
            assert_eq!(handle.future_quota_value(), Some(0));
        }
    );
}

#[test]
fn retained_validation_errors_keep_localized_field_details() {
    let violation = json!({
        "field": "schedule.cron.expr", "description": "invalid expression", "reason": "INVALID_CRON",
        "localizedMessage": {"locale": "es", "message": "Expresión inválida"}
    });
    retained_detail!(
        BadRequest,
        BadRequestOwnedView,
        BadRequestView<'static>,
        json!({"fieldViolations": [violation.clone()]}),
        |handle| {
            assert_eq!(handle.field_violations().len(), 1);
            assert_eq!(handle.field_violations()[0].field, "schedule.cron.expr");
        }
    );
    retained_detail!(
        bad_request::FieldViolation,
        bad_request::FieldViolationOwnedView,
        bad_request::FieldViolationView<'static>,
        violation,
        |handle| {
            assert_eq!(handle.field(), "schedule.cron.expr");
            assert_eq!(handle.description(), "invalid expression");
            assert_eq!(handle.reason(), "INVALID_CRON");
            assert!(handle.localized_message().is_set());
            assert_eq!(handle.localized_message().locale, "es");
            assert_eq!(handle.localized_message().message, "Expresión inválida");
        }
    );
    retained_detail!(
        Status,
        StatusOwnedView,
        StatusView<'static>,
        json!({"code": 3, "message": "invalid schedule"}),
        |handle| {
            assert_eq!(handle.code(), 3);
            assert_eq!(handle.message(), "invalid schedule");
            assert!(handle.details().is_empty());
        }
    );
}

#[test]
fn rpc_codes_keep_standard_names_numbers_and_closed_enum_validation() {
    let mut numbers: Vec<_> = Code::values().iter().map(Enumeration::to_i32).collect();
    numbers.sort_unstable();
    assert_eq!(numbers, (0..=16).collect::<Vec<_>>());
    assert_proto_sequence(
        vec![Code::INVALID_ARGUMENT, Code::UNAVAILABLE],
        json!(["INVALID_ARGUMENT", "UNAVAILABLE"]),
    );
    for (number, name) in [
        (0, "OK"),
        (1, "CANCELLED"),
        (2, "UNKNOWN"),
        (3, "INVALID_ARGUMENT"),
        (4, "DEADLINE_EXCEEDED"),
        (5, "NOT_FOUND"),
        (6, "ALREADY_EXISTS"),
        (7, "PERMISSION_DENIED"),
        (8, "RESOURCE_EXHAUSTED"),
        (9, "FAILED_PRECONDITION"),
        (10, "ABORTED"),
        (11, "OUT_OF_RANGE"),
        (12, "UNIMPLEMENTED"),
        (13, "INTERNAL"),
        (14, "UNAVAILABLE"),
        (15, "DATA_LOSS"),
        (16, "UNAUTHENTICATED"),
    ] {
        let code: Code = serde_json::from_value(json!(number)).expect("known numeric code");
        assert_eq!(code.to_i32(), number);
        assert_eq!(code.proto_name(), name);
        assert_eq!(Code::from_proto_name(name), Some(code));
        assert_eq!(serde_json::to_value(code).expect("code JSON"), json!(name));
        assert_eq!(serde_json::from_value::<Code>(json!(name)).expect("named code"), code);
    }
    assert_eq!(
        serde_json::from_value::<Code>(json!(null)).expect("null code"),
        Code::OK
    );
    for invalid in [
        json!(-1),
        json!(17),
        json!(i64::MIN),
        json!(u64::MAX),
        json!("FUTURE"),
        json!({}),
        json!(true),
    ] {
        assert!(serde_json::from_value::<Code>(invalid).is_err());
    }
}
