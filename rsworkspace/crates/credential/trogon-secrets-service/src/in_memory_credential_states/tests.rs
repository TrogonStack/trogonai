use trogon_credential_domain::{CredentialId, CredentialOwnerId};
use trogonai_proto::gateway::credentials::state_v1;

use super::*;

fn credential() -> CredentialKey {
    CredentialKey::new(
        CredentialOwnerId::new("project-1").unwrap(),
        CredentialId::new("0190a5b4-7c1e-7d2a-9f3b-6c8d4e2a1b0f").unwrap(),
    )
}

fn missing() -> CredentialStateSnapshot {
    CredentialStateSnapshot {
        state: Some(state_v1::CredentialMissingState {}.into()),
    }
}

#[tokio::test]
async fn a_failing_read_fails_and_is_counted() {
    let states = InMemoryCredentialStates::new();
    states.set(credential(), missing());
    let error = CredentialStateReadError::Unavailable {
        message: "no responders".to_owned(),
    };
    states.fail_with(error.clone());

    assert_eq!(states.read(&credential()).await, Err(error));
    assert_eq!(states.reads(), 1);
}

#[tokio::test]
async fn an_unset_credential_has_no_state() {
    let states = InMemoryCredentialStates::new();

    assert_eq!(states.read(&credential()).await, Ok(None));
}

#[tokio::test]
async fn a_set_credential_reads_its_state() {
    let states = InMemoryCredentialStates::new();
    states.set(credential(), missing());

    assert_eq!(states.read(&credential()).await, Ok(Some(missing())));
    assert_eq!(states.reads(), 1);
}
