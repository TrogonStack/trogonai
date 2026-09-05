//! Schema-only markers still expose generated codecs whose compatibility must
//! not depend on whether their current schema declares any payload fields.

use std::fmt::Debug;

use buffa::bytes::Bytes;
use buffa::{DecodeError, DecodeOptions, HasMessageView, Message, OwnedView, ViewEncode};
use serde_json::json;

use super::retained_fixture::retained_detail;
use super::{assert_json_codec, assert_malformed, assert_wire_codec};
use crate::decider::v1 as decider;

const FUTURE_WIRE: &[u8] = b"\x08\x96\x01\x11\x08\x07\x06\x05\x04\x03\x02\x01\x1a\x03a\xffb\x25\x04\x03\x02\x01";

fn assert_fieldless_wire_contract<M>()
where
    M: Message + HasMessageView + Debug + PartialEq,
    for<'a> M::View<'a>: ViewEncode<'a>,
{
    let empty = M::decode_from_slice(b"").expect("empty marker");
    assert_wire_codec(FUTURE_WIRE, &empty);
    let decoded = M::decode_from_slice(FUTURE_WIRE).expect("future fields");
    assert!(decoded.try_encode_to_vec().expect("canonical marker wire").is_empty());
    let view = M::decode_view(FUTURE_WIRE).expect("future fields view");
    assert!(view.try_encode_to_vec().expect("canonical view wire").is_empty());

    assert_malformed::<M>(b"\x00", DecodeError::InvalidFieldNumber);
    assert_malformed::<M>(b"\x0f", DecodeError::InvalidWireType(7));
    assert_malformed::<M>(b"\x08\x80", DecodeError::UnexpectedEof);
    assert_malformed::<M>(b"\x1a\x02x", DecodeError::UnexpectedEof);
    assert_malformed::<M>(b"\x11\x01", DecodeError::UnexpectedEof);
    assert_malformed::<M>(b"\x25\x01", DecodeError::UnexpectedEof);
}

macro_rules! fault_codec {
    ($name:ident, $owned:ident, $retained:ident, $view:ident) => {
        #[test]
        fn $name() {
            assert_fieldless_wire_contract::<decider::$owned>();
            retained_detail!(
                decider::$owned,
                decider::$retained,
                decider::$view<'static>,
                json!({}),
                |handle| {
                    assert!(handle.view().try_encode_to_vec().expect("canonical retained wire").is_empty());
                }
            );
            let retained = decider::$retained::decode(Bytes::copy_from_slice(FUTURE_WIRE))
                .expect("retained future fields");
            assert_eq!(serde_json::to_value(&retained).expect("schema-only JSON"), json!({}));
            assert_eq!(retained.into_bytes().as_ref(), FUTURE_WIRE);
            let future: decider::$owned = serde_json::from_value(json!({"futureField": "data"}))
                .expect("unknown JSON field");
            assert_eq!(serde_json::to_value(future).expect("canonical JSON"), json!({}));
        }
    };
}

fault_codec!(
    unroutable_marker_codec_preserves_future_wire,
    CommandTypeUnroutableError,
    CommandTypeUnroutableErrorOwnedView,
    CommandTypeUnroutableErrorView
);
fault_codec!(
    malformed_request_marker_codec_preserves_future_wire,
    CommandRequestMalformedError,
    CommandRequestMalformedErrorOwnedView,
    CommandRequestMalformedErrorView
);
fault_codec!(
    unsatisfiable_revision_marker_codec_preserves_future_wire,
    ExpectedRevisionUnsatisfiableError,
    ExpectedRevisionUnsatisfiableErrorOwnedView,
    ExpectedRevisionUnsatisfiableErrorView
);
fault_codec!(
    conflict_marker_codec_preserves_future_wire,
    StreamWriteConflictError,
    StreamWriteConflictErrorOwnedView,
    StreamWriteConflictErrorView
);
fault_codec!(
    guest_fault_marker_codec_preserves_future_wire,
    GuestFaultError,
    GuestFaultErrorOwnedView,
    GuestFaultErrorView
);
fault_codec!(
    deadline_marker_codec_preserves_future_wire,
    GuestDeadlineExceededError,
    GuestDeadlineExceededErrorOwnedView,
    GuestDeadlineExceededErrorView
);
fault_codec!(
    storage_marker_codec_preserves_future_wire,
    StorageUnavailableError,
    StorageUnavailableErrorOwnedView,
    StorageUnavailableErrorView
);
fault_codec!(
    host_fault_marker_codec_preserves_future_wire,
    HostInternalError,
    HostInternalErrorOwnedView,
    HostInternalErrorView
);
fault_codec!(
    admission_marker_codec_preserves_future_wire,
    AdmissionLimitReachedError,
    AdmissionLimitReachedErrorOwnedView,
    AdmissionLimitReachedErrorView
);
fault_codec!(
    missing_principal_marker_codec_preserves_future_wire,
    PrincipalMissingError,
    PrincipalMissingErrorOwnedView,
    PrincipalMissingErrorView
);
fault_codec!(
    unauthorized_principal_marker_codec_preserves_future_wire,
    PrincipalUnauthorizedError,
    PrincipalUnauthorizedErrorOwnedView,
    PrincipalUnauthorizedErrorView
);
