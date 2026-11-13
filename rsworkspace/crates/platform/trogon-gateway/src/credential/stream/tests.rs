use trogon_credential_domain::{CredentialIdentity, CredentialKind, CredentialScope, SourceKind};

use super::*;

fn owner(raw: &str) -> CredentialOwnerId {
    CredentialOwnerId::new(raw).unwrap()
}

fn stream_id(owner_id: &str) -> CredentialKey {
    CredentialIdentity::derive(
        CredentialScope::source(owner(owner_id), SourceKind::GitHub),
        CredentialKind::WebhookSecret,
    )
    .key()
}

#[test]
fn subject_sits_below_the_project_scope_with_the_opaque_id_as_the_last_token() {
    let stream_id = stream_id("tenant-1");
    let subject = credential_event_subject(&stream_id).unwrap();

    assert_eq!(
        subject.as_str(),
        format!(
            "{CREDENTIAL_EVENT_SUBJECT_PREFIX}.tenant-1.{}",
            stream_id.credential_id()
        )
    );
}

#[test]
fn resolver_scope_is_the_project_subtree() {
    let resolver = CredentialEventSubjectResolver::for_owner(&owner("tenant-1")).unwrap();

    assert_eq!(
        resolver.scope().as_prefix(),
        format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.tenant-1.")
    );
    assert_eq!(resolver.subject_scope(), Some(resolver.scope()));
}

#[test]
fn dotted_project_ids_stay_a_single_subject_token() {
    let resolver = CredentialEventSubjectResolver::for_owner(&owner("acme.prod")).unwrap();

    assert_eq!(
        resolver.scope().as_prefix(),
        format!("{CREDENTIAL_EVENT_SUBJECT_PREFIX}.acme~prod.")
    );
}

#[test]
fn a_dotted_project_cannot_reach_into_another_projects_subtree() {
    let dotted = credential_event_subject(&stream_id("acme.prod")).unwrap();
    let other = CredentialEventSubjectResolver::for_owner(&owner("acme")).unwrap();

    assert!(!dotted.as_str().starts_with(other.scope().as_prefix()));
}

#[test]
fn resolver_rejects_a_stream_id_from_another_project() {
    let resolver = CredentialEventSubjectResolver::for_owner(&owner("tenant-1")).unwrap();

    let error = resolver.subject_for(&stream_id("tenant-2")).unwrap_err();

    assert!(matches!(
        error,
        CredentialEventSubjectResolverError::OutsideProject { owner_id, .. } if owner_id.as_str() == "tenant-1"
    ));
}

#[test]
fn the_same_slot_in_two_projects_resolves_to_two_subjects() {
    let first = credential_event_subject(&stream_id("tenant-1")).unwrap();
    let second = credential_event_subject(&stream_id("tenant-2")).unwrap();

    assert_ne!(first.as_str(), second.as_str());
}

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
