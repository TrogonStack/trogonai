use std::fmt;

use async_trait::async_trait;
use base64::{Engine, engine::general_purpose::STANDARD};
use clap::ValueEnum;
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use ed25519_dalek::pkcs8::{DecodePrivateKey, DecodePublicKey};

use crate::manifest::{ManifestSignature, SIGNATURE_ALGORITHM};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SignerKind {
    File,
    Kms,
}

#[derive(Debug)]
pub enum SignerError {
    Io(std::io::Error),
    Parse(String),
    Unsupported(String),
}

impl fmt::Display for SignerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "signer io error: {error}"),
            Self::Parse(message) => write!(f, "signer parse error: {message}"),
            Self::Unsupported(message) => write!(f, "signer unsupported: {message}"),
        }
    }
}

impl std::error::Error for SignerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

#[async_trait]
pub trait ManifestSigner: Send + Sync {
    fn key_id(&self) -> &str;
    fn verifying_key(&self) -> VerifyingKey;
    fn sign_payload(&self, payload: &[u8]) -> Result<ManifestSignature, SignerError>;
}

pub mod file;
pub mod kms;

pub use file::FileManifestSigner;
pub use kms::KmsManifestSigner;

pub fn load_signer(kind: SignerKind, key_path: &std::path::Path) -> Result<Box<dyn ManifestSigner>, SignerError> {
    match kind {
        SignerKind::File => Ok(Box::new(FileManifestSigner::from_pem_path(key_path)?)),
        SignerKind::Kms => Ok(Box::new(KmsManifestSigner::new())),
    }
}

pub fn sign_payload_with_key(
    payload: &[u8],
    signing_key: &SigningKey,
    key_id: &str,
) -> Result<ManifestSignature, SignerError> {
    let signature = signing_key.sign(payload);
    Ok(ManifestSignature {
        algorithm: SIGNATURE_ALGORITHM.to_string(),
        key_id: key_id.to_string(),
        value: STANDARD.encode(signature.to_bytes()),
    })
}

pub fn verifying_key_from_pem(pem: &str) -> Result<VerifyingKey, SignerError> {
    if let Ok(signing_key) = SigningKey::from_pkcs8_pem(pem) {
        return Ok(signing_key.verifying_key());
    }
    VerifyingKey::from_public_key_pem(pem).map_err(|error| SignerError::Parse(error.to_string()))
}
