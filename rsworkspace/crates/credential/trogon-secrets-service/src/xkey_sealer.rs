use std::fmt;

use nkeys::XKey;
use trogon_std::SecretString;

use super::{RequesterXkey, SealError, SealedValue, Sealer};

/// Seals with the service's own xkey as the authenticated sender, per ADR#0083
/// Decision 1, so the requester can tell a value the service sealed from one
/// anyone else sealed to the same key.
#[derive(Clone)]
pub struct XkeySealer {
    service: XKey,
}

impl XkeySealer {
    /// Builds the sealer from the service's curve seed.
    pub fn from_seed(seed: &SecretString) -> Result<Self, XkeySealerError> {
        let service = XKey::from_seed(seed.as_str()).map_err(|_| XkeySealerError::NotACurveSeed)?;
        Ok(Self { service })
    }

    /// The public half of the service xkey, which a requester opens with.
    pub fn public_key(&self) -> String {
        self.service.public_key()
    }
}

impl Sealer for XkeySealer {
    fn seal(&self, value: &SecretString, recipient: &RequesterXkey) -> Result<SealedValue, SealError> {
        let sealed = self.service.seal(value.as_str().as_bytes(), recipient.key())?;
        Ok(SealedValue::new(sealed)?)
    }
}

impl fmt::Debug for XkeySealer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("XkeySealer").field(&self.service.public_key()).finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum XkeySealerError {
    #[error("service xkey seed is not a curve seed")]
    NotACurveSeed,
}

#[cfg(test)]
mod tests;
