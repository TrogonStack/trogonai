use super::*;

#[test]
fn stream_config_matches_credential_contract() {
    let config = stream_config();

    assert_eq!(config.name, CREDENTIAL_STREAM);
    assert_eq!(config.subjects, vec![format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.>")]);
    assert!(config.allow_atomic_publish);
}

#[test]
fn the_stream_config_keeps_every_event() {
    assert_eq!(validate_stream_config(&stream_config()), Ok(()));
}

#[test]
fn a_stream_reported_unlimited_keeps_every_event() {
    let reported = stream::Config {
        max_messages_per_subject: -1,
        max_messages: -1,
        max_bytes: -1,
        ..stream_config()
    };

    assert_eq!(validate_stream_config(&reported), Ok(()));
}

#[test]
fn each_setting_that_could_drop_events_is_refused() {
    let cases = [
        (
            stream::Config {
                storage: stream::StorageType::Memory,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::NotOnDisk,
        ),
        (
            stream::Config {
                retention: stream::RetentionPolicy::WorkQueue,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::DiscardsConsumedEvents,
        ),
        (
            stream::Config {
                retention: stream::RetentionPolicy::Interest,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::DiscardsConsumedEvents,
        ),
        (
            stream::Config {
                max_age: std::time::Duration::from_secs(3600),
                ..stream_config()
            },
            CredentialEventStreamMismatchError::ExpiresByAge,
        ),
        (
            stream::Config {
                max_messages_per_subject: 1,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::CapsEventsPerSubject,
        ),
        (
            stream::Config {
                max_messages: 1,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::CapsEvents,
        ),
        (
            stream::Config {
                max_bytes: 1,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::CapsBytes,
        ),
        (
            stream::Config {
                allow_message_ttl: true,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::ExpiresPerEvent,
        ),
        (
            stream::Config {
                allow_rollup: true,
                ..stream_config()
            },
            CredentialEventStreamMismatchError::RollsUpSubjects,
        ),
    ];

    for (config, expected) in cases {
        assert_eq!(validate_stream_config(&config), Err(expected));
    }
}

mod jetstream_tests;
