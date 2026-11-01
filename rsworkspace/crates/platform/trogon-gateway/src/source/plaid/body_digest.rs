use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaidBodyDigest(String);

impl PlaidBodyDigest {
    pub fn of(body: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(body)))
    }

    pub fn matches_claim(&self, claimed_hex: &str) -> bool {
        self.0.as_bytes().ct_eq(claimed_hex.as_bytes()).into()
    }
}

#[cfg(test)]
mod tests;
