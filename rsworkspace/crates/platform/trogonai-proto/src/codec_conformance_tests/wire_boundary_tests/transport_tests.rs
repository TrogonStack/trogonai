use super::assert_field_wire_types;
use crate::grpc_nats_micro::v1::{FailRequest, FailResponse, SayRequest, SayResponse};
use crate::nats::micro::v1alpha1::{MethodOptions, ServiceOptions};

#[test]
fn transport_fields_reject_wrong_types_and_truncation_before_exposing_a_view() {
    assert_field_wire_types::<SayRequest>(&[1], &[]);
    assert_field_wire_types::<SayResponse>(&[1], &[]);
    assert_field_wire_types::<FailRequest>(&[2], &[1]);
    assert_field_wire_types::<FailResponse>(&[1], &[]);
    assert_field_wire_types::<ServiceOptions>(&[3, 4, 5], &[6]);
    assert_field_wire_types::<MethodOptions>(&[3], &[]);
    assert_field_wire_types::<crate::content::v1alpha1::Content>(&[1, 2], &[]);
}
