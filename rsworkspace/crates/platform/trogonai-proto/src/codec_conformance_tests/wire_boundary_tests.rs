use buffa::{DecodeError, HasMessageView, Message};

use super::assert_malformed;
use crate::r#gen::trogon::error::v1alpha1::{FieldOptions, MessageOptions, message_options};

fn assert_field_wire_types<M: Message + HasMessageView>(length_delimited: &[u8], varints: &[u8]) {
    for (expected, fields) in [(2_u8, length_delimited), (0, varints)] {
        let actual = if expected == 2 { 0 } else { 2 };
        for &field in fields {
            assert_malformed::<M>(
                &[field << 3 | actual, 0],
                DecodeError::WireTypeMismatch {
                    field_number: u32::from(field),
                    expected,
                    actual,
                },
            );
            let incomplete = [field << 3 | expected, if expected == 2 { 1 } else { 0x80 }];
            assert_malformed::<M>(&incomplete, DecodeError::UnexpectedEof);
        }
    }
}

fn assert_embedded_validation<M: Message + HasMessageView>(fields: &[u8]) {
    for &field in fields {
        assert_malformed::<M>(&[field << 3 | 2, 1, 0], DecodeError::InvalidFieldNumber);
        assert_malformed::<M>(
            &[field << 3 | 2, 0, field << 3 | 2, 1, 0],
            DecodeError::InvalidFieldNumber,
        );
    }
}

#[test]
fn error_template_fields_enforce_their_wire_types_and_validate_nested_options() {
    assert_field_wire_types::<MessageOptions>(&[1], &[]);
    assert_embedded_validation::<MessageOptions>(&[1]);
    assert_field_wire_types::<message_options::Template>(&[1, 2, 3, 6, 7], &[4, 5]);
    assert_embedded_validation::<message_options::Template>(&[6, 7]);
    assert_field_wire_types::<message_options::HelpLink>(&[1, 2], &[]);
    assert_field_wire_types::<message_options::MetadataEntry>(&[1, 2], &[3]);
    assert_field_wire_types::<FieldOptions>(&[2, 3], &[1]);
    assert_field_wire_types::<crate::r#gen::elixirpb::FileOptions>(&[1], &[]);
}

#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
mod rpc_tests;

#[cfg(feature = "grpc-nats-micro")]
mod transport_tests;

#[cfg(feature = "schedules")]
mod scheduler_tests;
