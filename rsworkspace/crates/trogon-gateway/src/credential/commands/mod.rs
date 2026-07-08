#![allow(dead_code)]

mod activate_credential_rotation;
mod activate_credential_write;
pub(crate) mod domain;
mod record_credential_rotation_failure;
mod record_credential_write_failure;
mod request_credential_rotation;
mod request_credential_write;
mod revoke_credential;
pub(crate) mod snapshot;
pub(crate) mod state;
#[cfg(test)]
mod tests;

pub use activate_credential_rotation::ActivateCredentialRotation;
pub use activate_credential_write::ActivateCredentialWrite;
pub use record_credential_rotation_failure::RecordCredentialRotationFailure;
pub use record_credential_write_failure::RecordCredentialWriteFailure;
pub use request_credential_rotation::RequestCredentialRotation;
pub use request_credential_write::RequestCredentialWrite;
pub use revoke_credential::RevokeCredential;
pub(crate) use state::{CredentialDecideError, CredentialEvolveError, evolve, initial_state};
