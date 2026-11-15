use trogon_credential_domain::{CredentialId, CredentialKey, CredentialOwnerId};

use super::*;

fn credential() -> CredentialKey {
    CredentialKey::new(
        CredentialOwnerId::new("project.1").unwrap(),
        CredentialId::new("0f8fad5b-d9cb-469f-a165-70867728950e").unwrap(),
    )
}

#[test]
fn an_unknown_event_type_is_named() {
    let error = InvalidationSourceError::UnknownEventType {
        event_type: "acme.v1.Unknown".to_owned(),
    };

    assert_eq!(
        error.to_string(),
        "event type 'acme.v1.Unknown' is not a credential event this build knows"
    );
}

#[test]
fn an_event_from_another_subject_names_the_credential_and_the_subject() {
    let error = InvalidationSourceError::EventFromAnotherSubject {
        credential: credential(),
        subject: "credentials.events.v1.project-2.unused".to_owned(),
    };

    assert_eq!(
        error.to_string(),
        "event for credential 'project.1/0f8fad5b-d9cb-469f-a165-70867728950e' was appended on another \
         credential's subject 'credentials.events.v1.project-2.unused'"
    );
}

#[test]
fn an_event_at_the_epoch_names_its_sequence() {
    let error = InvalidationSourceError::NotAppendedAfterEpoch { sequence: 42 };

    assert_eq!(
        error.to_string(),
        "credential event at sequence 42 was appended no later than the Unix epoch"
    );
}
