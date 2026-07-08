#![allow(dead_code)]

mod activate_credential_rotation;
mod activate_credential_write;
pub(crate) mod credential_lifecycle_handler;
pub(crate) mod credential_lifecycle_store;
pub(crate) mod credential_lifecycle_stream;
pub mod domain;
mod record_credential_rotation_failure;
mod record_credential_write_failure;
mod request_credential_rotation;
mod request_credential_write;
mod revoke_credential;
mod snapshot;
mod state;
#[cfg(test)]
mod tests;

pub use activate_credential_rotation::ActivateCredentialRotation;
pub use activate_credential_write::ActivateCredentialWrite;
pub use domain::{CredentialFailureReason, CredentialLifecycleEvent, CredentialLifecycleEventPayloadError};
pub use record_credential_rotation_failure::RecordCredentialRotationFailure;
pub use record_credential_write_failure::RecordCredentialWriteFailure;
pub use request_credential_rotation::RequestCredentialRotation;
pub use request_credential_write::RequestCredentialWrite;
pub use revoke_credential::RevokeCredential;
pub(crate) use state::{
    ActiveCredential, CredentialLifecycleDecideError, CredentialLifecycleEvolveError, CredentialLifecycleState,
    PendingCredentialWrite, evolve, initial_state,
};
