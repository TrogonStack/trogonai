use std::fmt;
use std::sync::Arc;

use nkeys::XKey;

use super::ServiceXkeyError;

/// The secrets service's public curve key, pinned from boot material
/// (ADR#0033 Decision 5).
///
/// A released value is opened only with this key as its authenticated sender
/// (ADR#0083 Decision 1), so a value sealed to the requester by anyone else
/// never opens.
#[derive(Clone)]
pub struct ServiceXkey(XKey);

impl ServiceXkey {
    pub fn from_public_key(raw: &str) -> Result<Self, ServiceXkeyError> {
        XKey::from_public_key(raw)
            .map(Self)
            .map_err(|error| ServiceXkeyError::NotACurvePublicKey(Arc::new(error)))
    }

    pub fn public_key(&self) -> String {
        self.0.public_key()
    }

    /// Pins the key a stand-in service seals with.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn of(xkey: &XKey) -> Self {
        Self(xkey.clone())
    }

    pub(crate) fn key(&self) -> &XKey {
        &self.0
    }
}

impl fmt::Debug for ServiceXkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ServiceXkey").field(&self.0.public_key()).finish()
    }
}

#[cfg(test)]
mod tests;
