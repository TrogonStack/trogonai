#![allow(dead_code)]

mod commands;
pub mod domain;
pub(crate) mod handler;
pub(crate) mod processor;
mod snapshot;
mod state;
pub(crate) mod store;
pub(crate) mod stream;

pub use commands::{
    ActivateCredentialRotation, ActivateCredentialWrite, RecordCredentialRotationFailure, RecordCredentialWriteFailure,
    RequestCredentialRotation, RequestCredentialWrite, RevokeCredential,
};
pub use domain::{CredentialEvent, CredentialEventPayloadError, CredentialFailureReason};
pub(crate) use state::{
    ActiveCredential, CredentialDecideError, CredentialEvolveError, CredentialState, PendingCredentialWrite, evolve,
    initial_state,
};
