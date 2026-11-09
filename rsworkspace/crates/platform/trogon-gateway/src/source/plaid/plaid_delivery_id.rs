use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaidDeliveryId(String);

impl PlaidDeliveryId {
    pub fn of(token: &str) -> Self {
        Self(hex::encode(Sha256::digest(token.as_bytes())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests;
