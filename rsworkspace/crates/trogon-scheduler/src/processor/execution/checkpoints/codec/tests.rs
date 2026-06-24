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
