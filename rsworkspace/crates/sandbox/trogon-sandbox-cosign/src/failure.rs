use std::fmt::Display;

use crate::CosignError;

pub(crate) fn sign_failed<E: Display>(context: &'static str) -> impl FnOnce(E) -> CosignError {
    move |error| CosignError::SignFailed(format!("{context}: {error}"))
}

pub(crate) fn verify_failed<E: Display>(context: &'static str) -> impl FnOnce(E) -> CosignError {
    move |error| CosignError::VerifyFailed(format!("{context}: {error}"))
}

pub(crate) fn public_key_failed<E: Display>(context: &'static str) -> impl FnOnce(E) -> CosignError {
    move |error| CosignError::PublicKey(format!("{context}: {error}"))
}

pub(crate) fn registry_failed<E: Display>(context: &'static str) -> impl FnOnce(E) -> CosignError {
    move |error| CosignError::Registry(format!("{context}: {error}"))
}
