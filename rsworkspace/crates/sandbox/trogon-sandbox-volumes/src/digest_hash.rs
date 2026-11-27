use std::path::Path;

use sha2::{Digest as _, Sha256};
use trogon_sandbox_contract::Digest;

use crate::VolumeError;

pub(crate) fn hash_file(path: &Path) -> Result<Vec<u8>, VolumeError> {
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes).to_vec())
}

pub(crate) fn digest_from_hash(hash: &[u8]) -> Result<Digest, VolumeError> {
    Ok(Digest::new(format!("sha256:{}", hex::encode(hash)))?)
}
