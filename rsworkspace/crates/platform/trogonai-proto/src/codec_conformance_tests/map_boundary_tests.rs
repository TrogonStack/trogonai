use buffa::DecodeError;
use serde_json::json;

use super::{assert_json_codec, assert_malformed, assert_wire_codec};
use crate::google::rpc::{ErrorInfo, quota_failure};
#[cfg(feature = "grpc-nats-micro")]
use crate::nats::micro::v1alpha1::{MethodOptions, ServiceOptions};

macro_rules! map_entry_contract {
    ($message:ty, $tag:expr, $expected:expr) => {{
        let expected = assert_json_codec::<$message>($expected);
        assert_wire_codec(&[$tag, 2, 0x18, 1], &expected);
        for (entry_tag, field_number) in [(0x08, 1), (0x10, 2)] {
            assert_malformed::<$message>(
                &[$tag, 2, entry_tag, 0],
                DecodeError::WireTypeMismatch {
                    field_number,
                    expected: 2,
                    actual: 0,
                },
            );
        }
        for entry_tag in [0x0a, 0x12] {
            assert_malformed::<$message>(&[$tag, 3, entry_tag, 1, 0xff], DecodeError::InvalidUtf8);
        }
        assert_malformed::<$message>(&[$tag, 2, 0x18, 0x80], DecodeError::UnexpectedEof);
    }};
}

#[test]
fn rpc_metadata_maps_skip_future_entry_fields_but_validate_keys_and_values() {
    map_entry_contract!(ErrorInfo, 0x1a, json!({"metadata": {"": ""}}));
    map_entry_contract!(quota_failure::Violation, 0x32, json!({"quotaDimensions": {"": ""}}));
}

#[cfg(feature = "grpc-nats-micro")]
#[test]
fn discovery_maps_skip_future_entry_fields_but_validate_keys_and_values() {
    map_entry_contract!(ServiceOptions, 0x2a, json!({"metadata": {"": ""}}));
    map_entry_contract!(MethodOptions, 0x1a, json!({"metadata": {"": ""}}));
}
