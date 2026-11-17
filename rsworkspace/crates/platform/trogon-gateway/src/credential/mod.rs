#![allow(dead_code)]

pub(crate) mod commands;
pub(crate) mod constants;
mod ensure_event_stream;
mod ensure_event_stream_error;
pub(crate) mod event_stream;
mod event_stream_mismatch_error;
pub(crate) mod store;
pub(crate) mod stream;

pub(crate) use commands::CredentialDecideError;
pub use commands::{
    ActivateCredentialRotation, ActivateCredentialWrite, CompleteCredentialDestroy, RecordCredentialDestroyFailure,
    RecordCredentialRotationFailure, RecordCredentialWriteFailure, RequestCredentialDestroy, RequestCredentialRotation,
    RequestCredentialWrite, RevokeCredential,
};
pub(crate) use trogon_credential_aggregate::{CredentialEvolveError, evolve_stream, initial_state};
