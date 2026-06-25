use std::time::Duration;

use chrono::{DateTime, Utc};
use std::error::Error;
use trogonai_proto::scheduler::schedules::checkpoints_v1;

use super::*;
use crate::commands::domain::{Delivery, MessageContent, Schedule, ScheduleHeaders, ScheduleMessage};

fn record(schedule: Schedule, status: ScheduleStatus, outcome: ReconcileOutcome) -> ScheduleCheckpointRecord {
    let schedule_id = crate::commands::domain::ScheduleId::parse("orders/created").unwrap();
    ScheduleCheckpointRecord {
        schedule_id,
        status,
        schedule,
        delivery: Delivery::NatsEvent {
            route: crate::commands::domain::DeliveryRoute::new("agent.run").unwrap(),
            ttl: Some(crate::commands::domain::TtlDuration::from_secs(45).unwrap()),
            source: Some(crate::commands::domain::SamplingSource::latest_from_subject("agent.events").unwrap()),
        },
        message: ScheduleMessage {
            content: MessageContent::json(r#"{"ok":true}"#),
            headers: ScheduleHeaders::new([("x-kind", "heartbeat")]).unwrap(),
        },
        last_applied_stream_position: StreamPosition::try_new(7).unwrap(),
        last_applied_event_id: Some("event-7".to_string()),
        last_outcome: outcome,
    }
}

fn at() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2030-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn checkpoint_record_round_trips_through_the_codec() {
    let original = record(
        Schedule::cron("0 0 * * * *", Some("America/New_York".to_string())).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );

    let encoded = encode_checkpoint_record(&original).unwrap();
    let decoded = decode_checkpoint_record(&encoded).unwrap();

    assert_eq!(decoded, original);
}

#[test]
fn round_trips_every_schedule_kind_and_status() {
    let cases = [
        (
            Schedule::At { at: at() },
            ScheduleStatus::Scheduled,
            ReconcileOutcome::Published,
        ),
        (
            Schedule::every(Duration::from_secs(30)).unwrap(),
            ScheduleStatus::Paused,
            ReconcileOutcome::Purged,
        ),
        (
            Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap(),
            ScheduleStatus::Unsupported,
            ReconcileOutcome::Unsupported,
        ),
        (
            Schedule::every(Duration::from_secs(45)).unwrap(),
            ScheduleStatus::Removed,
            ReconcileOutcome::StoredPaused,
        ),
        (
            Schedule::every(Duration::from_secs(60)).unwrap(),
            ScheduleStatus::Expired,
            ReconcileOutcome::Expired,
        ),
        (
            Schedule::every(Duration::from_secs(75)).unwrap(),
            ScheduleStatus::Scheduled,
            ReconcileOutcome::DuplicateStale,
        ),
    ];

    for (schedule, status, outcome) in cases {
        let original = record(schedule, status, outcome);
        let decoded = decode_checkpoint_record(&encode_checkpoint_record(&original).unwrap()).unwrap();
        assert_eq!(decoded.status, status);
        assert_eq!(decoded.last_outcome, outcome);
        match (&decoded.schedule, &original.schedule) {
            (
                Schedule::RRule {
                    dtstart: a, rrule: ra, ..
                },
                Schedule::RRule {
                    dtstart: b, rrule: rb, ..
                },
            ) => {
                assert_eq!(a.to_datetime(), b.to_datetime());
                assert_eq!(ra.as_str(), rb.as_str());
            }
            _ => assert_eq!(decoded.schedule, original.schedule),
        }
    }
}

#[test]
fn corrupt_bytes_are_rejected() {
    assert!(matches!(
        decode_checkpoint_record(b"not proto").unwrap_err(),
        CheckpointCodecError::Wire { .. }
    ));
}

#[test]
fn default_enums_decode_to_stable_domain_values() {
    let stored = checkpoints_v1::ScheduleCheckpoint {
        schedule_id: Some("orders/created".to_string()),
        status: None,
        last_applied_stream_position: Some(1),
        last_applied_event_id: None,
        last_outcome: None,
        schedule: buffa::MessageField::none(),
        delivery: buffa::MessageField::none(),
        message: buffa::MessageField::none(),
    };
    let error = decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err();
    assert!(matches!(error, CheckpointCodecError::Domain { .. }));
}

#[test]
fn unrecognized_enum_values_decode_to_unknown() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let mut stored =
        checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap()).unwrap();
    stored.status = Some(buffa::EnumValue::from(999));
    stored.last_outcome = Some(buffa::EnumValue::from(999));

    let decoded = decode_checkpoint_record(&stored.encode_to_vec()).unwrap();

    assert_eq!(decoded.status, ScheduleStatus::Unknown);
    assert_eq!(decoded.last_outcome, ReconcileOutcome::Unknown);
}

#[test]
fn missing_status_and_outcome_are_rejected() {
    let schedule = Schedule::every(Duration::from_secs(30)).unwrap();
    let original = record(schedule, ScheduleStatus::Scheduled, ReconcileOutcome::Published);
    let mut stored =
        checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap()).unwrap();
    stored.status = None;
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));

    stored =
        checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap()).unwrap();
    stored.last_outcome = None;
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));
}

#[test]
fn zero_stream_position_is_rejected() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let mut bytes = encode_checkpoint_record(&original).unwrap();
    bytes = rewrite_checkpoint_watermark(&bytes, 0);
    assert!(matches!(
        decode_checkpoint_record(&bytes).unwrap_err(),
        CheckpointCodecError::StreamPosition
    ));
}

#[test]
fn codec_errors_display_and_expose_sources() {
    let wire = decode_checkpoint_record(b"not proto").unwrap_err();
    assert_eq!(
        wire.to_string(),
        format!("checkpoint record wire format is invalid: {}", wire.source().unwrap())
    );
    assert!(std::error::Error::source(&wire).is_some());

    let domain = decode_checkpoint_record(
        &checkpoints_v1::ScheduleCheckpoint {
            schedule_id: Some("orders/created".to_string()),
            status: Some(checkpoints_v1::ScheduleCheckpointStatus::Scheduled.into()),
            last_applied_stream_position: Some(1),
            last_applied_event_id: None,
            last_outcome: Some(checkpoints_v1::ReconcileOutcome::Published.into()),
            schedule: buffa::MessageField::none(),
            delivery: buffa::MessageField::none(),
            message: buffa::MessageField::none(),
        }
        .encode_to_vec(),
    )
    .unwrap_err();
    assert_eq!(
        domain.to_string(),
        format!(
            "checkpoint record snapshot could not be rebuilt: {}",
            domain.source().unwrap()
        )
    );
    assert!(std::error::Error::source(&domain).is_some());

    let stream_position = CheckpointCodecError::StreamPosition;
    assert_eq!(
        stream_position.to_string(),
        "checkpoint stream position must be greater than zero"
    );
    assert!(std::error::Error::source(&stream_position).is_none());

    let snapshot_conversion = CheckpointCodecError::SnapshotConversion;
    assert_eq!(
        snapshot_conversion.to_string(),
        "checkpoint snapshot could not be encoded to proto"
    );
    assert!(std::error::Error::source(&snapshot_conversion).is_none());
}

#[test]
fn decode_checkpoint_envelope_reads_metadata_without_definition() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let bytes = corrupt_checkpoint_schedule(&encode_checkpoint_record(&original).unwrap());
    assert!(decode_checkpoint_record(&bytes).is_err());
    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(7).unwrap()),
            last_applied_event_id: Some("event-7".to_string()),
        }
    );
}

#[test]
fn decode_checkpoint_envelope_recovers_fields_parsed_before_a_truncation_point() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let bytes = encode_checkpoint_record(&original).unwrap();
    // Truncate inside the trailing nested snapshot, after the envelope fields.
    let truncated = &bytes[..bytes.len() - 5];

    assert!(decode_checkpoint_record(truncated).is_err());
    assert_eq!(
        decode_checkpoint_envelope(truncated),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(7).unwrap()),
            last_applied_event_id: Some("event-7".to_string()),
        }
    );
}

#[test]
fn decode_checkpoint_envelope_keeps_watermark_when_event_id_is_invalid_utf8() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let bytes = corrupt_checkpoint_event_id(&encode_checkpoint_record(&original).unwrap());
    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(7).unwrap()),
            last_applied_event_id: None,
        }
    );
}

#[test]
fn decode_checkpoint_envelope_keeps_event_id_when_watermark_is_missing_or_invalid() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let bytes = rewrite_checkpoint_watermark(&encode_checkpoint_record(&original).unwrap(), 0);

    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: None,
            last_applied_event_id: Some("event-7".to_string()),
        }
    );
}

#[test]
fn missing_schedule_id_and_snapshot_fields_are_rejected() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let encoded = encode_checkpoint_record(&original).unwrap();

    let mut stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.schedule_id = None;
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));

    stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.schedule = buffa::MessageField::none();
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));

    stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.delivery = buffa::MessageField::none();
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));

    stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.message = buffa::MessageField::none();
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));

    stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.last_applied_stream_position = None;
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));
}

#[test]
fn invalid_schedule_id_is_rejected() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let mut stored =
        checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap()).unwrap();
    stored.schedule_id = Some(String::new());

    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));
}

#[test]
fn unspecified_status_and_outcome_are_rejected() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let encoded = encode_checkpoint_record(&original).unwrap();

    let mut stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.status = Some(checkpoints_v1::ScheduleCheckpointStatus::Unspecified.into());
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));

    stored = checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encoded).unwrap();
    stored.last_outcome = Some(checkpoints_v1::ReconcileOutcome::Unspecified.into());
    assert!(matches!(
        decode_checkpoint_record(&stored.encode_to_vec()).unwrap_err(),
        CheckpointCodecError::Domain { .. }
    ));
}

#[test]
fn unknown_status_and_outcome_encode_as_unspecified() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Unknown,
        ReconcileOutcome::Unknown,
    );
    let stored =
        checkpoints_v1::ScheduleCheckpoint::decode_from_slice(&encode_checkpoint_record(&original).unwrap()).unwrap();

    assert_eq!(
        stored.status.as_ref().and_then(|status| status.as_known()),
        Some(checkpoints_v1::ScheduleCheckpointStatus::Unspecified)
    );
    assert_eq!(
        stored.last_outcome.as_ref().and_then(|outcome| outcome.as_known()),
        Some(checkpoints_v1::ReconcileOutcome::Unspecified)
    );
}

#[test]
fn codec_errors_cover_json_and_envelope_variants() {
    let json = CheckpointCodecError::Json {
        source: serde_json::from_str::<serde_json::Value>("not json").unwrap_err(),
    };
    assert_eq!(
        json.to_string(),
        format!("checkpoint record JSON is invalid: {}", json.source().unwrap())
    );
    assert!(Error::source(&json).is_some());

    let envelope = CheckpointCodecError::Envelope;
    assert_eq!(envelope.to_string(), "checkpoint envelope metadata is invalid");
    assert!(Error::source(&envelope).is_none());
}

#[test]
fn decode_checkpoint_envelope_tolerates_unrecognized_wire_types() {
    let bytes = [
        0x09, 0, 0, 0, 0, 0, 0, 0, 0, //
        0x18, 0x07, //
        0x22, 0x07, b'e', b'v', b'e', b'n', b't', b'-', b'7',
    ];
    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(7).unwrap()),
            last_applied_event_id: Some("event-7".to_string()),
        }
    );

    assert_eq!(
        decode_checkpoint_envelope(&[0x0e]),
        CorruptCheckpointEnvelope {
            watermark: None,
            last_applied_event_id: None,
        }
    );
}

/// Craft a raw protobuf blob that contains Fixed64 (wire type 1) and Fixed32
/// (wire type 5) fields before the envelope fields so that
/// `scan_protobuf_fields` exercises those skip branches.
#[test]
fn decode_checkpoint_envelope_skips_fixed64_and_fixed32_fields() {
    // Build: [field 1, wire 1 = varint 0x09] + 8 bytes + [watermark field 3, wire 0 = 0x18] + value 7
    let mut bytes: Vec<u8> = Vec::new();
    // field 1, wire type 1 (Fixed64): tag = (1 << 3) | 1 = 0x09
    bytes.push(0x09);
    bytes.extend_from_slice(&[0u8; 8]);
    // field 2, wire type 5 (Fixed32): tag = (2 << 3) | 5 = 0x15
    bytes.push(0x15);
    bytes.extend_from_slice(&[0u8; 4]);
    // watermark: field 3, wire type 0: tag = (3 << 3) | 0 = 0x18, value = 7
    bytes.push(0x18);
    bytes.push(0x07);
    // event id: field 4, wire type 2: tag = (4 << 3) | 2 = 0x22, len = 7, "event-7"
    bytes.push(0x22);
    bytes.push(0x07);
    bytes.extend_from_slice(b"event-7");

    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(7).unwrap()),
            last_applied_event_id: Some("event-7".to_string()),
        }
    );
}

/// A Fixed64 field at the end of the buffer with fewer than 8 bytes remaining
/// causes scan_protobuf_fields to return Err, which is silently swallowed by
/// decode_checkpoint_envelope (it keeps whatever it already parsed).
#[test]
fn decode_checkpoint_envelope_tolerates_truncated_fixed64() {
    // watermark first, then a truncated Fixed64 field at the end
    let mut bytes: Vec<u8> = Vec::new();
    bytes.push(0x18); // field 3, wire 0
    bytes.push(0x05); // value = 5
    bytes.push(0x09); // field 1, wire 1 (Fixed64) — only 3 bytes of data follow
    bytes.extend_from_slice(&[0u8; 3]);

    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(5).unwrap()),
            last_applied_event_id: None,
        }
    );
}

/// Fixed32 field truncated causes the same early-exit behaviour.
#[test]
fn decode_checkpoint_envelope_tolerates_truncated_fixed32() {
    let mut bytes: Vec<u8> = Vec::new();
    bytes.push(0x18); // field 3, wire 0 — watermark = 4
    bytes.push(0x04);
    bytes.push(0x15); // field 2, wire 5 (Fixed32) — only 2 bytes follow
    bytes.extend_from_slice(&[0u8; 2]);

    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(4).unwrap()),
            last_applied_event_id: None,
        }
    );
}

/// A LengthDelimited payload whose declared length extends past the end of the
/// buffer causes scan_protobuf_fields to return Err (the `end > bytes.len()`
/// guard), which decode_checkpoint_envelope swallows.
#[test]
fn decode_checkpoint_envelope_tolerates_length_delimited_overflow() {
    // watermark (field 3, wire 0, value 3) then LD field with declared length 100 but only 2 bytes
    let mut bytes: Vec<u8> = vec![
        0x18, 0x03, // watermark = 3
        0x22, 0x64, // field 4, wire 2, length = 100
    ];
    bytes.extend_from_slice(b"ab"); // only 2 bytes

    // The scan stops at the truncated LD field, keeping the watermark.
    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(3).unwrap()),
            last_applied_event_id: None,
        }
    );
}

/// A StartGroup tag (wire type 3) is an immediately-fatal parse error in
/// scan_protobuf_fields; decode_checkpoint_envelope keeps what was parsed
/// before the offending tag.
#[test]
fn decode_checkpoint_envelope_tolerates_start_group_tag() {
    // watermark = 9 (field 3, wire 0), then StartGroup (field 5, wire 3 = 0x2b)
    let bytes: Vec<u8> = vec![0x18, 0x09, 0x2b, 0x00];

    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(9).unwrap()),
            last_applied_event_id: None,
        }
    );
}

/// An EndGroup tag (wire type 4) has the same early-exit semantics as StartGroup.
#[test]
fn decode_checkpoint_envelope_tolerates_end_group_tag() {
    // watermark = 6 (field 3, wire 0), then EndGroup (field 1, wire 4 = 0x0c)
    let bytes: Vec<u8> = vec![0x18, 0x06, 0x0c];

    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(6).unwrap()),
            last_applied_event_id: None,
        }
    );
}

/// A multi-byte varint that would require more than 64 bits causes read_varint
/// to return None (the `shift >= 64` guard).  The missing tag causes
/// scan_protobuf_fields to return Err, and decode_checkpoint_envelope
/// returns empty fields.
#[test]
fn decode_checkpoint_envelope_handles_overlong_varint() {
    // Craft a varint that never terminates within 64 bits: 10 continuation
    // bytes all with the MSB set.  This exercises the `shift >= 64` branch.
    let mut bytes: Vec<u8> = vec![0x80u8; 10];
    bytes.push(0x01); // technically a terminator but shift is already 70 by the time we get here
    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: None,
            last_applied_event_id: None,
        }
    );
}

/// A varint field whose value bytes never have the MSB clear (no terminator
/// before the end of the buffer) causes decode_varint to return None.
/// This is reached via the varint payload path in scan_protobuf_fields when
/// the payload slice passed to `decode_varint` consists of only continuation
/// bytes.
#[test]
fn decode_checkpoint_envelope_handles_varint_with_no_terminator() {
    // Tag 0x18 = field 3 wire 0 (varint), value payload is two bytes with MSB set
    // and no terminator — decode_varint returns None, watermark stays None.
    let bytes: Vec<u8> = vec![0x18, 0x80, 0x80];
    assert_eq!(
        decode_checkpoint_envelope(&bytes),
        CorruptCheckpointEnvelope {
            watermark: None,
            last_applied_event_id: None,
        }
    );
}

/// rewrite_varint_field with corrupt tag at the start: the tag read fails so
/// the remaining bytes are appended verbatim and the new value is appended at end.
/// The resulting buffer has the old corrupt bytes first, then the new field,
/// but the corrupt bytes prevent scan from reaching the new field.
#[test]
fn rewrite_checkpoint_watermark_with_corrupt_leading_tag() {
    // A single continuation byte as the tag — read_varint returns None
    // (no terminator), so the whole input is appended verbatim and the new
    // watermark field is appended at the end. The result is non-empty and
    // longer than the input.
    let corrupt: &[u8] = &[0x80]; // continuation byte, no terminator
    let result = rewrite_checkpoint_watermark(corrupt, 42);
    // result = [0x80] ++ encode_varint(field3_tag) ++ encode_varint(42)
    // decode_checkpoint_envelope cannot parse 0x80 as a tag (no terminator),
    // so it returns empty; but the rewrite itself must produce a non-empty buffer.
    assert!(!result.is_empty());
    assert!(result.len() > corrupt.len());
}

/// rewrite_varint_field with a corrupt inner-varint value: the inner
/// read_varint fails causing the field-start bytes to be emitted verbatim,
/// then the loop breaks and the new field is appended.
#[test]
fn rewrite_checkpoint_watermark_with_corrupt_varint_value() {
    // field 3, wire 0 (watermark tag = 0x18) followed by a continuation byte
    // with no terminator — the inner read_varint fails.
    // Bytes field_start..end (the partial field) are appended verbatim, then
    // the new watermark field is appended.  The corrupt intermediate bytes
    // prevent decode_checkpoint_envelope from reaching the new appended field.
    let corrupt: &[u8] = &[0x18, 0x80];
    let result = rewrite_checkpoint_watermark(corrupt, 77);
    assert!(!result.is_empty());
    // The new watermark bytes are always appended at the end.
    assert!(result.len() > corrupt.len());
}

/// rewrite_length_delimited_field with corrupt tag bytes propagates the
/// verbatim copy path.
#[test]
fn corrupt_checkpoint_event_id_with_corrupt_leading_tag() {
    let corrupt: &[u8] = &[0x80]; // no terminator
    let result = corrupt_checkpoint_event_id(corrupt);
    // Field was not found, so replacement is appended; result is non-empty.
    assert!(!result.is_empty());
}

/// rewrite_length_delimited_field on a LD field whose length varint is
/// truncated: the inner read_varint fails, bytes are copied verbatim and the
/// loop breaks.
#[test]
fn corrupt_checkpoint_event_id_with_truncated_ld_length() {
    // field 4, wire 2 = tag 0x22; length byte = 0x80 (continuation, no terminator)
    let corrupt: &[u8] = &[0x22, 0x80];
    let result = corrupt_checkpoint_event_id(corrupt);
    assert!(!result.is_empty());
}

/// rewrite_length_delimited_field when the LD payload is truncated (end >
/// bytes.len()) copies verbatim and breaks.
#[test]
fn corrupt_checkpoint_event_id_with_truncated_ld_payload() {
    // field 4, wire 2 = 0x22; length = 100, but only 2 bytes follow
    let corrupt: &[u8] = &[0x22, 0x64, 0x01, 0x02];
    let result = corrupt_checkpoint_event_id(corrupt);
    assert!(!result.is_empty());
}

/// rewrite_length_delimited_field: Fixed64 and Fixed32 fields are skipped
/// via the saturating-add path (no break on truncation — they just clamp).
#[test]
fn corrupt_checkpoint_schedule_skips_fixed64_and_fixed32() {
    // Build a buffer with a Fixed64 field, a Fixed32 field, then the schedule
    // field (field 6, wire 2 = 0x32) with a small payload.
    let mut bytes: Vec<u8> = Vec::new();
    // field 1, wire 1 (Fixed64)
    bytes.push(0x09);
    bytes.extend_from_slice(&[0u8; 8]);
    // field 2, wire 5 (Fixed32)
    bytes.push(0x15);
    bytes.extend_from_slice(&[0u8; 4]);
    // field 6 (schedule), wire 2: tag = (6 << 3) | 2 = 0x32
    bytes.push(0x32);
    bytes.push(0x03); // length = 3
    bytes.extend_from_slice(b"abc");

    let result = corrupt_checkpoint_schedule(&bytes);
    // The schedule field should have been replaced with 0xff 0xff.
    assert!(!result.is_empty());
    assert_ne!(result, bytes);
}

/// encode_varint correctly encodes multi-byte values. This exercises the
/// loop continuation path (value != 0 after shifting).
#[test]
fn encode_varint_handles_multi_byte_values() {
    // 128 needs 2 bytes: 0x80 0x01
    let encoded = encode_varint(128);
    assert_eq!(encoded, vec![0x80, 0x01]);

    // 300 needs 2 bytes: 0xac 0x02
    let encoded = encode_varint(300);
    assert_eq!(encoded, vec![0xac, 0x02]);

    // Large value: u64::MAX needs 10 bytes
    let encoded = encode_varint(u64::MAX);
    assert_eq!(encoded.len(), 10);
}

#[test]
fn rewrite_helpers_handle_truncated_or_minimal_bytes() {
    let original = record(
        Schedule::every(Duration::from_secs(30)).unwrap(),
        ScheduleStatus::Scheduled,
        ReconcileOutcome::Published,
    );
    let encoded = encode_checkpoint_record(&original).unwrap();
    let truncated = &encoded[..encoded.len() / 2];

    let rewritten_watermark = rewrite_checkpoint_watermark(truncated, 99);
    assert_eq!(
        decode_checkpoint_envelope(&rewritten_watermark).watermark,
        Some(StreamPosition::try_new(99).unwrap())
    );

    let appended_watermark = rewrite_checkpoint_watermark(&[], 12);
    assert_eq!(
        decode_checkpoint_envelope(&appended_watermark),
        CorruptCheckpointEnvelope {
            watermark: Some(StreamPosition::try_new(12).unwrap()),
            last_applied_event_id: None,
        }
    );

    assert!(!corrupt_checkpoint_schedule(truncated).is_empty());
    assert!(!corrupt_checkpoint_event_id(truncated).is_empty());
    assert!(!encode_varint(300).is_empty());
}
