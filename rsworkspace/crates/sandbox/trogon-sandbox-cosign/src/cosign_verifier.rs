use std::fs;
use std::path::Path;

use crate::CosignError;
use crate::failure::public_key_failed;

/// PEM public key used to verify Cosign signatures on pulled snapshots.
#[derive(Debug, Clone)]
pub struct CosignVerifier {
    pub(crate) public_key_pem: Vec<u8>,
    pub(crate) insecure_registries: Vec<String>,
}

impl CosignVerifier {
    pub fn from_pem_path(path: impl AsRef<Path>) -> Result<Self, CosignError> {
        let public_key_pem = fs::read(path.as_ref()).map_err(public_key_failed("read public key"))?;
        Ok(Self::from_pem(public_key_pem))
    }

    pub fn from_pem(public_key_pem: Vec<u8>) -> Self {
        Self {
            public_key_pem,
            insecure_registries: Vec::new(),
        }
    }

    pub fn with_insecure_registries(mut self, registries: Vec<String>) -> Self {
        self.insecure_registries = registries;
        self
    }

    pub fn public_key_pem(&self) -> &[u8] {
        &self.public_key_pem
    }
}
