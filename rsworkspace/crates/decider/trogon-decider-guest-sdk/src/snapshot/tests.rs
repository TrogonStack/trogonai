use super::*;
use std::convert::Infallible;
use std::error::Error as _;
use trogon_decider::{Decider, Decision, WritePrecondition};
use trogonai_proto::scheduler::schedules::state_v1::State;

struct SnapshotDecider;

impl Decider for SnapshotDecider {
    type StreamId = str;
    type State = State;
    type Event = State;
    type DecideError = Infallible;
    type EvolveError = Infallible;
    const WRITE_PRECONDITION: WritePrecondition = WritePrecondition::StreamUnchanged;

    fn stream_id(&self) -> &str {
        "snapshot-fixture"
    }

    fn initial_state() -> State {
        State::default().with_last_occurrence_sequence(7)
    }

    fn evolve(_state: State, event: &State) -> Result<State, Infallible> {
        Ok(event.clone())
    }

    fn decide(state: &State, _command: &Self) -> Result<Decision<Self>, Infallible> {
        Ok(Decision::event(state.clone()))
    }
}

#[test]
fn snapshot_encoding_matches_versioned_protobuf_frame() {
    let state = State::default().with_last_occurrence_sequence(150);
    let expected = b"\x0a\x02v1\x12\x03\x18\x96\x01";
    assert_eq!(encode_snapshot(&state, "v1"), expected);
    assert_eq!(encode_current(&state, "v1"), expected);
    assert_eq!(decode_snapshot::<State>(expected, "v1").unwrap(), state);
}

#[test]
fn snapshot_round_trip_preserves_presence_and_multibyte_schema_length() {
    let state = State::default()
        .with_last_occurrence_sequence(u64::MAX)
        .with_completed(false);
    let schema = "s".repeat(130);
    let frame = encode_snapshot(&state, &schema);
    assert_eq!(&frame[..3], &[0x0a, 0x82, 0x01]);
    assert_eq!(decode_snapshot::<State>(&frame, &schema).unwrap(), state);
}

#[test]
fn absent_snapshot_uses_decider_initial_state_and_present_snapshot_restores_state() {
    let initial = load_or_initial::<SnapshotDecider, State>(None, "v1");
    assert_eq!(initial.last_occurrence_sequence, Some(7));

    let saved = State::default().with_last_occurrence_sequence(150).with_completed(true);
    let restored = load_or_initial::<SnapshotDecider, State>(Some(encode_current(&saved, "v1")), "v1");
    assert_eq!(restored, saved);
}

#[test]
fn present_empty_state_does_not_fall_back_to_initial_state() {
    let restored = load_or_initial::<SnapshotDecider, State>(Some(b"\x0a\x02v1\x12\x00".to_vec()), "v1");
    assert_eq!(restored, State::default());
    assert_ne!(restored, SnapshotDecider::initial_state());
}

#[test]
fn corrupt_or_incompatible_snapshot_traps_instead_of_resetting_state() {
    for frame in [
        vec![],
        b"\x0a\x02v2\x12\x00".to_vec(),
        b"\x0a\x02v1\x12\x01\x18".to_vec(),
    ] {
        assert!(std::panic::catch_unwind(|| load_or_initial::<SnapshotDecider, State>(Some(frame), "v1")).is_err());
    }
}

#[test]
fn snapshot_accepts_reordered_fields_and_unknown_protobuf_fields() {
    let frame = [
        0x18, 0x96, 0x01, 0x21, 1, 2, 3, 4, 5, 6, 7, 8, 0x12, 0x03, 0x18, 0x96, 0x01, 0x2a, 0x03, b'a', b'b', b'c',
        0x35, 1, 2, 3, 4, 0x0a, 0x02, b'v', b'1',
    ];
    assert_eq!(
        decode_snapshot::<State>(&frame, "v1").unwrap().last_occurrence_sequence,
        Some(150)
    );
}

#[test]
fn snapshot_repeated_singular_fields_use_the_last_value() {
    let frame = b"\x0a\x02v0\x12\x02\x18\x01\x0a\x02v1\x12\x02\x18\x02";
    assert_eq!(
        decode_snapshot::<State>(frame, "v1").unwrap().last_occurrence_sequence,
        Some(2)
    );
}

#[test]
fn snapshot_schema_mismatch_retains_expected_and_actual_versions() {
    let error = decode_snapshot::<State>(b"\x0a\x02v0\x12\x00", "v1").unwrap_err();
    assert!(
        matches!(&error, SnapshotDecodeError::SchemaMismatch { expected, actual } if expected == "v1" && actual == "v0")
    );
    assert!(error.source().is_none());
}

#[test]
fn snapshot_requires_both_fields_with_their_length_delimited_wire_types() {
    for frame in [b"".as_slice(), b"\x12\x00", b"\x08\x01\x12\x00"] {
        assert!(matches!(
            decode_snapshot::<State>(frame, "v1"),
            Err(SnapshotDecodeError::MissingSchemaVersion)
        ));
    }
    for frame in [b"\x0a\x02v1".as_slice(), b"\x0a\x02v1\x10\x00"] {
        assert!(matches!(
            decode_snapshot::<State>(frame, "v1"),
            Err(SnapshotDecodeError::MissingPayload)
        ));
    }
}

#[test]
fn snapshot_preserves_typed_utf8_and_payload_decode_errors() {
    let schema_error = decode_snapshot::<State>(b"\x0a\x01\xff\x12\x00", "v1").unwrap_err();
    assert!(matches!(schema_error, SnapshotDecodeError::SchemaVersionUtf8(_)));
    assert!(schema_error.source().unwrap().is::<std::string::FromUtf8Error>());

    let payload_error = decode_snapshot::<State>(b"\x0a\x02v1\x12\x01\x18", "v1").unwrap_err();
    assert!(matches!(payload_error, SnapshotDecodeError::Payload(_)));
    assert!(payload_error.source().unwrap().is::<buffa::DecodeError>());
}

#[test]
fn snapshot_rejects_truncated_keys_lengths_and_unknown_fields() {
    for frame in [
        b"\x80".as_slice(),
        b"\x0a\x80",
        b"\x0a\x02v",
        b"\x18\x80",
        b"\x21\x01\x02\x03\x04\x05\x06\x07",
        b"\x2a\x02x",
        b"\x35\x01\x02\x03",
    ] {
        assert!(matches!(
            decode_snapshot::<State>(frame, "v1"),
            Err(SnapshotDecodeError::UnexpectedEof)
        ));
    }
}

#[test]
fn snapshot_rejects_overflowing_varints_and_field_lengths() {
    for frame in [
        &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02][..],
        &[0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80],
    ] {
        assert!(matches!(
            decode_snapshot::<State>(frame, "v1"),
            Err(SnapshotDecodeError::VarintOverflow)
        ));
    }
    let frame = [0x0a, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(matches!(
        decode_snapshot::<State>(&frame, "v1"),
        Err(SnapshotDecodeError::LengthOverflow)
    ));
}

#[test]
fn snapshot_rejects_unsupported_unknown_wire_types() {
    for tag in [0x1b, 0x1c, 0x1e, 0x1f] {
        assert!(matches!(
            decode_snapshot::<State>(&[tag], "v1"),
            Err(SnapshotDecodeError::UnsupportedWireType)
        ));
    }
}

#[test]
fn read_varint_decodes_multibyte_value() {
    // 150 encodes as 0x96 0x01.
    assert!(matches!(read_varint(&[0x96, 0x01], 0), Ok((150, 2))));
}

#[test]
fn read_varint_accepts_max_u64() {
    // u64::MAX is a 10-byte varint terminating in 0x01.
    let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];
    assert!(matches!(read_varint(&bytes, 0), Ok((u64::MAX, 10))));
}

#[test]
fn read_varint_rejects_overflow_in_tenth_byte() {
    // A 10th byte payload above 0x01 would truncate, so it must be rejected.
    let bytes = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];
    assert!(matches!(
        read_varint(&bytes, 0),
        Err(SnapshotDecodeError::VarintOverflow)
    ));
}

#[test]
fn read_varint_rejects_truncated_input() {
    assert!(matches!(
        read_varint(&[0x80], 0),
        Err(SnapshotDecodeError::UnexpectedEof)
    ));
}
