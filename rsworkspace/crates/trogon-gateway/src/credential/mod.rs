#![allow(dead_code)]

pub(crate) mod commands;
pub(crate) mod handler;
pub(crate) mod processor;
pub(crate) mod store;
pub(crate) mod stream;

pub use commands::domain::{CredentialEvent, CredentialEventPayloadError, CredentialFailureReason};
pub use commands::{
    ActivateCredentialRotation, ActivateCredentialWrite, RecordCredentialRotationFailure, RecordCredentialWriteFailure,
    RequestCredentialRotation, RequestCredentialWrite, RevokeCredential,
};
pub(crate) use commands::{
    ActiveCredential, CredentialDecideError, CredentialEvolveError, CredentialState, PendingCredentialWrite, evolve,
    initial_state,
};
