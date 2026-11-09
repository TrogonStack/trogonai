use std::fmt;

use sha2::{Digest, Sha256};

use crate::credential::commands::domain::{CredentialFingerprint, CredentialFingerprintError};

pub(super) fn digest(
    prefix: &str,
    input: impl fmt::Display,
) -> Result<CredentialFingerprint, CredentialFingerprintError> {
    CredentialFingerprint::new(format!("{prefix}:{}", hex::encode(Sha256::digest(input.to_string()))))
}

#[cfg(test)]
mod tests;
