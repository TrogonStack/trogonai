use super::{assert_embedded_validation, assert_field_wire_types};
use crate::google::rpc::{
    BadRequest, DebugInfo, ErrorInfo, Help, LocalizedMessage, PreconditionFailure, QuotaFailure, RequestInfo,
    ResourceInfo, RetryInfo, Status, bad_request, help, precondition_failure, quota_failure,
};

#[test]
fn rpc_detail_fields_reject_wrong_types_and_truncation_before_exposing_a_view() {
    assert_field_wire_types::<ErrorInfo>(&[1, 2, 3], &[]);
    assert_field_wire_types::<RetryInfo>(&[1], &[]);
    assert_field_wire_types::<DebugInfo>(&[1, 2], &[]);
    assert_field_wire_types::<QuotaFailure>(&[1], &[]);
    assert_field_wire_types::<quota_failure::Violation>(&[1, 2, 3, 4, 5, 6], &[7, 8]);
    assert_field_wire_types::<PreconditionFailure>(&[1], &[]);
    assert_field_wire_types::<precondition_failure::Violation>(&[1, 2, 3], &[]);
    assert_field_wire_types::<BadRequest>(&[1], &[]);
    assert_field_wire_types::<bad_request::FieldViolation>(&[1, 2, 3, 4], &[]);
    assert_field_wire_types::<RequestInfo>(&[1, 2], &[]);
    assert_field_wire_types::<ResourceInfo>(&[1, 2, 3, 4], &[]);
    assert_field_wire_types::<Help>(&[1], &[]);
    assert_field_wire_types::<help::Link>(&[1, 2], &[]);
    assert_field_wire_types::<LocalizedMessage>(&[1, 2], &[]);
    assert_field_wire_types::<Status>(&[2, 3], &[1]);
}

#[test]
fn rpc_details_validate_embedded_messages_before_exposing_a_view() {
    assert_embedded_validation::<RetryInfo>(&[1]);
    assert_embedded_validation::<QuotaFailure>(&[1]);
    assert_embedded_validation::<PreconditionFailure>(&[1]);
    assert_embedded_validation::<BadRequest>(&[1]);
    assert_embedded_validation::<bad_request::FieldViolation>(&[4]);
    assert_embedded_validation::<Help>(&[1]);
    assert_embedded_validation::<Status>(&[3]);
}
