use sha2::{Digest, Sha256};
use spiffe::X509Bundle;

use crate::MaterialError;
use crate::constants::MAX_ROOTS;

#[derive(PartialEq, Eq)]
pub(crate) struct BundleRoots(Vec<[u8; 32]>);

impl BundleRoots {
    pub fn new(bundle: &X509Bundle) -> Result<Self, MaterialError> {
        if bundle.authorities().len() > MAX_ROOTS {
            return Err(MaterialError::Bounds);
        }
        let mut hashes: Vec<[u8; 32]> = bundle
            .authorities()
            .iter()
            .map(|certificate| Sha256::digest(certificate.as_bytes()).into())
            .collect();
        hashes.sort_unstable();
        if hashes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(MaterialError::Identity);
        }
        Ok(Self(hashes))
    }
}
