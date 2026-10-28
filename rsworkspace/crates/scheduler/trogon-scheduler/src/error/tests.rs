use super::*;
use crate::commands::domain::MessageHeadersError as CommandMessageHeadersError;
use trogon_decider_nats::{
    JetStreamStoreError, OptimisticConcurrencyConflictError, ReadStreamError, SnapshotCodecError, SnapshotKvError,
    SnapshotStoreError, StreamStoreError,
};
use trogon_decider_runtime::{SnapshotTypeName, StreamPosition, StreamWritePrecondition};
use trogon_nats::SubjectTokenViolationError;

fn position(value: u64) -> StreamPosition {
    StreamPosition::try_new(value).expect("test stream position must be non-zero")
}

#[test]
fn from_command_message_headers_error_maps_to_spec_error() {
    let invalid_name = ScheduleSpecError::from(CommandMessageHeadersError::InvalidName {
        name: "bad name".to_string(),
    });
    assert!(matches!(invalid_name, ScheduleSpecError::InvalidHeaderName { .. }));

    let invalid_value = ScheduleSpecError::from(CommandMessageHeadersError::InvalidValue {
        name: "x-kind".to_string(),
    });
    assert!(matches!(invalid_value, ScheduleSpecError::InvalidHeaderValue { .. }));
}

#[test]
fn from_jetstream_resolve_subject_and_codec_pass_through() {
    let resolve: SchedulerError =
        JetStreamStoreError::<SchedulerError>::ResolveSubject(SchedulerError::ScheduleNotFound {
            id: "alpha".to_string(),
        })
        .into();
    assert!(matches!(resolve, SchedulerError::ScheduleNotFound { .. }));

    let codec: SchedulerError = JetStreamStoreError::<SchedulerError>::Codec(SchedulerError::ScheduleAlreadyExists {
        id: "alpha".to_string(),
    })
    .into();
    assert!(matches!(codec, SchedulerError::ScheduleAlreadyExists { .. }));
}

#[test]
fn from_jetstream_occ_maps_both_position_cases() {
    let with_position: SchedulerError = JetStreamStoreError::<SchedulerError>::OptimisticConcurrencyConflict(
        OptimisticConcurrencyConflictError::WithPosition {
            stream_id: "alpha".to_string(),
            expected: StreamWritePrecondition::At(position(3)),
            current_position: position(4),
        },
    )
    .into();
    assert!(matches!(
        with_position,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: Some(_),
            ..
        }
    ));

    let no_position: SchedulerError = JetStreamStoreError::<SchedulerError>::OptimisticConcurrencyConflict(
        OptimisticConcurrencyConflictError::NoPosition {
            stream_id: "alpha".to_string(),
            expected: StreamWritePrecondition::NoStream,
        },
    )
    .into();
    assert!(matches!(
        no_position,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: None,
            ..
        }
    ));

    // Exercise the Display path (and `occ_position_detail`) for both position cases.
    assert!(!with_position.to_string().is_empty());
    assert!(!no_position.to_string().is_empty());
}

#[test]
fn from_snapshot_invalid_key_maps_to_event_error() {
    let direct: SchedulerError =
        SnapshotStoreError::<std::convert::Infallible, std::convert::Infallible>::InvalidSnapshotKey {
            key: "weird".to_string(),
        }
        .into();
    assert!(matches!(direct, SchedulerError::Event { .. }));

    let via_store: SchedulerError =
        JetStreamStoreError::<SchedulerError>::Snapshot(SnapshotStoreError::InvalidSnapshotKey {
            key: "weird".to_string(),
        })
        .into();
    assert!(matches!(via_store, SchedulerError::Event { .. }));
}

#[test]
fn snapshot_storage_and_codec_errors_keep_their_typed_causes() {
    let kv_error: SchedulerError =
        SnapshotStoreError::<std::convert::Infallible>::Kv(SnapshotKvError::ReadSnapshotValue {
            source: async_nats::jetstream::kv::EntryError::new(async_nats::jetstream::kv::EntryErrorKind::Other),
        })
        .into();
    assert!(matches!(kv_error, SchedulerError::Kv { .. }));
    assert!(std::error::Error::source(&kv_error).unwrap().is::<SnapshotKvError>());

    let codec_error: SchedulerError = SnapshotStoreError::<std::io::Error>::Codec(SnapshotCodecError::DecodePayload {
        source: std::io::Error::other("corrupt snapshot payload"),
    })
    .into();
    assert!(matches!(codec_error, SchedulerError::Event { .. }));
    assert!(
        std::error::Error::source(&codec_error)
            .unwrap()
            .is::<SnapshotCodecError<std::io::Error>>()
    );

    let missing_checkpoint: SchedulerError = SnapshotStoreError::<std::convert::Infallible>::MissingCheckpointName {
        snapshot_type: SnapshotTypeName::new("tests.scheduler.State").unwrap(),
    }
    .into();
    assert!(matches!(missing_checkpoint, SchedulerError::Event { .. }));
    assert!(std::error::Error::source(&missing_checkpoint).is_some());
}

#[test]
fn stream_failures_preserve_causes_and_scope_violations_remain_schedule_errors() {
    let read: SchedulerError = JetStreamStoreError::<SchedulerError>::ReadStream(StreamStoreError::Read(
        ReadStreamError::MissingMessageHeader {
            header_name: "Nats-Msg-Id",
        },
    ))
    .into();
    assert!(matches!(read, SchedulerError::Event { .. }));
    assert!(matches!(
        std::error::Error::source(&read)
            .unwrap()
            .downcast_ref::<StreamStoreError>(),
        Some(StreamStoreError::Read(ReadStreamError::MissingMessageHeader { .. }))
    ));

    let append: SchedulerError =
        JetStreamStoreError::<SchedulerError>::AppendStream(StreamStoreError::WrongExpectedVersion).into();
    assert!(matches!(append, SchedulerError::Event { .. }));
    assert!(matches!(
        std::error::Error::source(&append)
            .unwrap()
            .downcast_ref::<StreamStoreError>(),
        Some(StreamStoreError::WrongExpectedVersion)
    ));

    let scope: SchedulerError = JetStreamStoreError::<SchedulerError>::SubjectOutsideScope {
        subject: "other.events.schedule".to_string(),
        scope: "scheduler.schedules.events.>".to_string(),
    }
    .into();
    assert!(matches!(scope, SchedulerError::Schedule { .. }));
    assert!(std::error::Error::source(&scope).is_some());
}

#[test]
fn invalid_schedule_spec_display_mentions_field() {
    let error = SchedulerError::invalid_schedule_spec(ScheduleSpecError::TtlMustBePositive);
    assert_eq!(error.to_string(), "Invalid schedule spec: ttl_sec must be >= 1");
}

#[test]
fn kv_source_preserves_source() {
    let error = SchedulerError::kv_source("open bucket", std::io::Error::other("boom"));
    assert_eq!(error.to_string(), "KV error: open bucket: boom");
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn scheduler_error_display_and_sources_cover_remaining_variants() {
    let event = SchedulerError::event_source("replay", std::io::Error::other("broken"));
    assert_eq!(event.to_string(), "Event error: replay: broken");
    assert!(std::error::Error::source(&event).is_some());

    let lease = SchedulerError::lease_source("renew", std::io::Error::other("lost"));
    assert_eq!(lease.to_string(), "Lease error: renew: lost");
    assert!(std::error::Error::source(&lease).is_some());

    let schedule = SchedulerError::schedule_source("upsert", std::io::Error::other("rejected"));
    assert_eq!(schedule.to_string(), "Schedule error: upsert: rejected");
    assert!(std::error::Error::source(&schedule).is_some());

    let already_exists = SchedulerError::ScheduleAlreadyExists {
        id: "job-1".to_string(),
    };
    assert_eq!(already_exists.to_string(), "Schedule 'job-1' already exists");
    assert!(std::error::Error::source(&already_exists).is_none());

    let not_found = SchedulerError::ScheduleNotFound {
        id: "missing".to_string(),
    };
    assert_eq!(not_found.to_string(), "Schedule 'missing' not found");
    assert!(std::error::Error::source(&not_found).is_none());

    let occ_missing = SchedulerError::OptimisticConcurrencyConflict {
        id: "job-1".to_string(),
        expected: StreamWritePrecondition::NoStream,
        current_position: None,
    };
    assert!(matches!(
        occ_missing,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: None,
            ..
        }
    ));
    assert!(std::error::Error::source(&occ_missing).is_none());

    let occ_current = SchedulerError::OptimisticConcurrencyConflict {
        id: "job-1".to_string(),
        expected: StreamWritePrecondition::At(position(3)),
        current_position: Some(position(4)),
    };
    assert!(matches!(
        &occ_current,
        SchedulerError::OptimisticConcurrencyConflict {
            current_position: Some(current),
            ..
        } if *current == position(4)
    ));
    assert!(std::error::Error::source(&occ_current).is_none());

    let serde_error: SchedulerError = serde_json::from_str::<serde_json::Value>("{").unwrap_err().into();
    assert!(matches!(&serde_error, SchedulerError::Serde(_)));
    assert!(std::error::Error::source(&serde_error).is_some());
}

#[test]
fn job_spec_error_display_and_sources_cover_remaining_variants() {
    let invalid_id = ScheduleSpecError::InvalidId {
        id: "".to_string(),
        source: SubjectTokenViolationError::Empty,
    };
    assert!(matches!(&invalid_id, ScheduleSpecError::InvalidId { id, .. } if id.is_empty()));
    assert!(std::error::Error::source(&invalid_id).is_some());

    let every = ScheduleSpecError::EverySecondsMustBePositive;
    assert_eq!(every.to_string(), "every_sec must be >= 1");

    let invalid_cron = ScheduleSpecError::InvalidCronExpression {
        expr: "bad cron".to_string(),
        source: Box::new(std::io::Error::other("invalid fields")),
    };
    assert!(matches!(&invalid_cron, ScheduleSpecError::InvalidCronExpression { expr, .. } if expr == "bad cron"));
    assert!(std::error::Error::source(&invalid_cron).is_some());

    let timezone = ScheduleSpecError::InvalidTimezone {
        timezone: "Mars/Base".to_string(),
    };
    assert_eq!(timezone.to_string(), "timezone 'Mars/Base' is invalid");
    assert!(std::error::Error::source(&timezone).is_none());

    let route = ScheduleSpecError::InvalidRoute {
        route: "agent.>".to_string(),
        source: SubjectTokenViolationError::InvalidCharacter('>'),
    };
    assert!(matches!(&route, ScheduleSpecError::InvalidRoute { route, .. } if route == "agent.>"));
    assert!(std::error::Error::source(&route).is_some());

    let sampling = ScheduleSpecError::InvalidSamplingSource {
        subject: "sensors.>".to_string(),
        source: SubjectTokenViolationError::InvalidCharacter('>'),
    };
    assert!(matches!(&sampling, ScheduleSpecError::InvalidSamplingSource { subject, .. } if subject == "sensors.>"));
    assert!(std::error::Error::source(&sampling).is_some());

    let invalid_header_name = ScheduleSpecError::InvalidHeaderName { name: "\n".to_string() };
    assert_eq!(invalid_header_name.to_string(), "header name '\n' is invalid");

    let reserved = ScheduleSpecError::ReservedHeaderName {
        name: "Nats-Schedule".to_string(),
    };
    assert_eq!(
        reserved.to_string(),
        "header name 'Nats-Schedule' is reserved by the scheduler"
    );

    let invalid_header_value = ScheduleSpecError::InvalidHeaderValue {
        name: "x-kind".to_string(),
    };
    assert_eq!(
        invalid_header_value.to_string(),
        "header 'x-kind' contains an invalid value"
    );
    assert!(std::error::Error::source(&invalid_header_value).is_none());
}
