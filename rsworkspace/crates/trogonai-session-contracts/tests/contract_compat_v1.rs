mod support;

use buffa::{ExtensionSet, Message as _, UnknownField, UnknownFieldData};
use support::{minimal_session_event_wire, sample_session_event};
use trogonai_session_contracts::{
    ContractValidationError, SCHEMA_VERSION_V1, SessionEvent, ValidatedSessionEvent,
};

#[test]
fn n_minus_one_reader_accepts_minimal_event_without_optional_fields() {
    let minimal = minimal_session_event_wire();
    let bytes = minimal.encode_to_vec();
    let decoded = SessionEvent::decode_from_slice(&bytes).expect("decode minimal event");

    let validated = ValidatedSessionEvent::try_from_event(decoded).expect("validate minimal event");
    assert_eq!(validated.schema_version, SCHEMA_VERSION_V1);
    assert_eq!(validated.event_id.as_str(), "evt_compat_minimal");
    assert_eq!(validated.session_id.as_str(), "sess_compat");
    assert_eq!(validated.seq.as_u64(), 7);
}

#[test]
fn n_minus_one_reader_accepts_schema_version_zero_as_v1() {
    let mut event = sample_session_event();
    event.schema_version = 0;

    let validated = ValidatedSessionEvent::try_from_event(event).expect("accept schema_version=0");
    assert_eq!(validated.schema_version, SCHEMA_VERSION_V1);
}

#[test]
fn rejects_unsupported_future_schema_version() {
    let mut event = sample_session_event();
    event.schema_version = 99;

    let err = ValidatedSessionEvent::try_from_event(event).expect_err("reject unknown version");
    assert_eq!(
        err,
        ContractValidationError::UnsupportedSchemaVersion {
            expected: SCHEMA_VERSION_V1,
            actual: 99,
        }
    );
}

#[test]
fn rejects_invalid_identifier_prefixes() {
    let mut event = sample_session_event();
    event.event_id = "bad_event".to_string();

    let err = ValidatedSessionEvent::try_from_event(event).expect_err("reject bad event_id");
    assert!(matches!(
        err,
        ContractValidationError::InvalidEventId(_)
    ));
}

#[test]
fn rejects_zero_seq() {
    let mut event = sample_session_event();
    event.seq = 0;

    let err = ValidatedSessionEvent::try_from_event(event).expect_err("reject zero seq");
    assert!(matches!(err, ContractValidationError::InvalidSeq(_)));
}

#[test]
fn roundtrip_preserves_unknown_fields_for_forward_compatibility() {
    let mut event = sample_session_event();
    event.unknown_fields_mut().push(UnknownField {
        number: 999,
        data: UnknownFieldData::Varint(1),
    });

    let bytes = event.encode_to_vec();
    let decoded = SessionEvent::decode_from_slice(&bytes).expect("decode with unknown field");

    let unknown = decoded
        .unknown_fields()
        .iter()
        .find(|field| field.number == 999)
        .expect("preserve unknown field");
    assert_eq!(unknown.data, UnknownFieldData::Varint(1));
    assert!(ValidatedSessionEvent::try_from_event(decoded).is_ok());
}
