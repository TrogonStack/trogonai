#![allow(dead_code)]

pub(crate) mod commands;
pub(crate) mod handler;
pub(crate) mod processor;
pub(crate) mod proto;
pub(crate) mod store;
pub(crate) mod stream;

pub use commands::domain::CredentialFailureReason;
pub use commands::{
    ActivateCredentialRotation, ActivateCredentialWrite, CompleteCredentialDestroy, RecordCredentialDestroyFailure,
    RecordCredentialRotationFailure, RecordCredentialWriteFailure, RequestCredentialDestroy, RequestCredentialRotation,
    RequestCredentialWrite, RevokeCredential,
};
pub(crate) use commands::{CredentialDecideError, CredentialEvolveError, evolve, initial_state};
