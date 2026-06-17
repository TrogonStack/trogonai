mod support;

use std::path::PathBuf;

use buffa::Message as _;
use support::{
    sample_artifact_metadata, sample_capability_schema, sample_context_twin,
    sample_model_switched_event, sample_session_event, sample_session_snapshot,
    sample_switch_outcome_recorded_event, write_golden_fixtures,
};
use trogonai_session_contracts::{
    ArtifactMetadata, CapabilitySchema, ContextTwin, SessionEvent, SessionSnapshot,
    ValidatedArtifactMetadata, ValidatedSessionEvent, ValidatedSessionSnapshot,
};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v1")
}

const GOLDEN_FIXTURES: [&str; 7] = [
    "session_event_session_created_v1.bin",
    "session_event_model_switched_v1.bin",
    "session_event_switch_outcome_recorded_v1.bin",
    "context_twin_v1.bin",
    "artifact_metadata_v1.bin",
    "capability_schema_v1.bin",
    "session_snapshot_v1.bin",
];

#[test]
fn regenerate_golden_fixtures_when_missing() {
    let dir = fixture_dir();
    let missing = !dir.exists()
        || GOLDEN_FIXTURES
            .iter()
            .any(|name| !dir.join(name).is_file());
    if missing {
        write_golden_fixtures(&dir);
    }
}

#[test]
fn golden_session_event_session_created_roundtrip() {
    let bytes = std::fs::read(fixture_dir().join("session_event_session_created_v1.bin"))
        .expect("read golden fixture");
    let decoded = SessionEvent::decode_from_slice(&bytes).expect("decode golden session event");
    let expected = sample_session_event();

    assert_eq!(decoded, expected);
    assert!(ValidatedSessionEvent::try_from_event(decoded).is_ok());
    assert_eq!(expected.encode_to_vec(), bytes);
}

#[test]
fn golden_session_event_model_switched_roundtrip() {
    let bytes = std::fs::read(fixture_dir().join("session_event_model_switched_v1.bin"))
        .expect("read golden fixture");
    let decoded = SessionEvent::decode_from_slice(&bytes).expect("decode golden model switch event");
    let expected = sample_model_switched_event();

    assert_eq!(decoded, expected);
    assert!(ValidatedSessionEvent::try_from_event(decoded).is_ok());
    assert_eq!(expected.encode_to_vec(), bytes);
}

#[test]
fn golden_session_event_switch_outcome_recorded_roundtrip() {
    let bytes = std::fs::read(fixture_dir().join("session_event_switch_outcome_recorded_v1.bin"))
        .expect("read golden fixture");
    let decoded = SessionEvent::decode_from_slice(&bytes).expect("decode golden switch outcome event");
    let expected = sample_switch_outcome_recorded_event();

    assert_eq!(decoded, expected);
    assert!(ValidatedSessionEvent::try_from_event(decoded).is_ok());
    assert_eq!(expected.encode_to_vec(), bytes);
}

#[test]
fn golden_context_twin_roundtrip() {
    let bytes =
        std::fs::read(fixture_dir().join("context_twin_v1.bin")).expect("read golden fixture");
    let decoded = ContextTwin::decode_from_slice(&bytes).expect("decode golden context twin");
    let expected = sample_context_twin();

    assert_eq!(decoded, expected);
    assert_eq!(expected.encode_to_vec(), bytes);
}

#[test]
fn golden_artifact_metadata_roundtrip() {
    let bytes =
        std::fs::read(fixture_dir().join("artifact_metadata_v1.bin")).expect("read golden fixture");
    let decoded =
        ArtifactMetadata::decode_from_slice(&bytes).expect("decode golden artifact metadata");
    let expected = sample_artifact_metadata();

    assert_eq!(decoded, expected);
    assert!(ValidatedArtifactMetadata::try_from_metadata(decoded).is_ok());
    assert_eq!(expected.encode_to_vec(), bytes);
}

#[test]
fn golden_capability_schema_roundtrip() {
    let bytes =
        std::fs::read(fixture_dir().join("capability_schema_v1.bin")).expect("read golden fixture");
    let decoded =
        CapabilitySchema::decode_from_slice(&bytes).expect("decode golden capability schema");
    let expected = sample_capability_schema();

    assert_eq!(decoded, expected);
    assert_eq!(expected.encode_to_vec(), bytes);
}

#[test]
fn golden_session_snapshot_roundtrip() {
    let bytes =
        std::fs::read(fixture_dir().join("session_snapshot_v1.bin")).expect("read golden fixture");
    let decoded = SessionSnapshot::decode_from_slice(&bytes).expect("decode golden snapshot");
    let expected = sample_session_snapshot();

    assert_eq!(decoded, expected);
    assert!(ValidatedSessionSnapshot::try_from_snapshot(decoded).is_ok());
    assert_eq!(expected.encode_to_vec(), bytes);
}
