use super::*;

#[cfg(not(coverage))]
mod command_execution_tests;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test position must be non-zero")
}

#[test]
fn builder_entrypoint_type_checks() {
    let _builder: fn(jetstream::Context, jetstream::stream::Stream, kv::Store) -> JetStreamStoreBuilder =
        JetStreamStore::builder;
}

#[test]
fn stream_read_from_maps_beginning_to_first_sequence() {
    assert_eq!(stream_read_from_to_sequence(ReadFrom::Beginning), 1);
}

#[test]
fn stream_read_from_maps_position_to_sequence() {
    assert_eq!(stream_read_from_to_sequence(ReadFrom::Position(position(42))), 42);
}

#[test]
fn expected_subject_sequence_allows_any_position_without_guard() {
    assert_eq!(
        resolve_expected_last_subject_sequence::<str, std::io::Error>(
            "jobs.backup",
            StreamWritePrecondition::Any,
            Some(position(7)),
        )
        .unwrap(),
        None
    );
}

#[test]
fn expected_subject_sequence_requires_existing_stream_before_publish() {
    assert_eq!(
        resolve_expected_last_subject_sequence::<str, std::io::Error>(
            "jobs.backup",
            StreamWritePrecondition::StreamExists,
            Some(position(7)),
        )
        .unwrap(),
        None
    );

    let error = resolve_expected_last_subject_sequence::<str, std::io::Error>(
        "jobs.backup",
        StreamWritePrecondition::StreamExists,
        None,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        JetStreamStoreError::OptimisticConcurrencyConflict(
            OptimisticConcurrencyConflictError::NoPosition { stream_id, expected }
        ) if stream_id == "jobs.backup"
            && expected == StreamWritePrecondition::StreamExists
    ));
}

#[test]
fn expected_subject_sequence_uses_nats_no_stream_guard() {
    assert_eq!(
        resolve_expected_last_subject_sequence::<str, std::io::Error>(
            "jobs.backup",
            StreamWritePrecondition::NoStream,
            None,
        )
        .unwrap(),
        Some(0)
    );
}

#[test]
fn expected_subject_sequence_uses_exact_subject_position() {
    assert_eq!(
        resolve_expected_last_subject_sequence::<str, std::io::Error>(
            "jobs.backup",
            StreamWritePrecondition::At(position(9)),
            Some(position(12)),
        )
        .unwrap(),
        Some(9)
    );
}
