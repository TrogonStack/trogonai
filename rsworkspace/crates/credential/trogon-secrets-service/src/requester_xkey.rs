use std::fmt;

use nkeys::XKey;

/// The curve public key a released value is sealed to.
///
/// Holds no private key: the requester keeps that, so only the requester can
/// open what the service seals.
#[derive(Clone)]
pub struct RequesterXkey {
    key: XKey,
    encoded: String,
}

impl RequesterXkey {
    pub fn parse(raw: &str) -> Result<Self, RequesterXkeyError> {
        let key = XKey::from_public_key(raw).map_err(|_| RequesterXkeyError::NotACurveKey)?;
        Ok(Self {
            key,
            encoded: raw.to_owned(),
        })
    }

    pub fn key(&self) -> &XKey {
        &self.key
    }

    pub fn as_str(&self) -> &str {
        &self.encoded
    }
}

impl PartialEq for RequesterXkey {
    fn eq(&self, other: &Self) -> bool {
        self.encoded == other.encoded
    }
}

impl Eq for RequesterXkey {}

impl fmt::Debug for RequesterXkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("RequesterXkey").field(&self.encoded).finish()
    }
}

impl fmt::Display for RequesterXkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.encoded)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RequesterXkeyError {
    #[error("requester xkey is not a curve public key")]
    NotACurveKey,
}

#[cfg(test)]
mod tests;
