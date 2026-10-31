use std::fmt;

use nkeys::XKey;

use super::xkey_public::XkeyPublic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CalloutXkeyError {
    #[error("is not an NKey curve seed")]
    NotACurveSeed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum XkeyExchangeError {
    #[error("the counterpart xkey is not a usable curve public key")]
    InvalidCounterpart,
    #[error("the sealed payload does not open with the callout xkey")]
    CannotOpen,
    #[error("the payload cannot be sealed to the counterpart xkey")]
    CannotSeal,
}

#[derive(Clone)]
pub struct CalloutXkey(XKey);

impl CalloutXkey {
    pub fn from_seed(seed: &str) -> Result<Self, CalloutXkeyError> {
        XKey::from_seed(seed.trim())
            .map(Self)
            .map_err(|_| CalloutXkeyError::NotACurveSeed)
    }

    pub fn public_key(&self) -> XkeyPublic {
        XkeyPublic::from_xkey(&self.0)
    }

    pub(crate) fn open(&self, sealed: &[u8], sender: &XkeyPublic) -> Result<Vec<u8>, XkeyExchangeError> {
        let sender = sender.to_xkey().map_err(|_| XkeyExchangeError::InvalidCounterpart)?;
        self.0.open(sealed, &sender).map_err(|_| XkeyExchangeError::CannotOpen)
    }

    pub(crate) fn seal(&self, plain: &[u8], recipient: &XkeyPublic) -> Result<Vec<u8>, XkeyExchangeError> {
        let recipient = recipient.to_xkey().map_err(|_| XkeyExchangeError::InvalidCounterpart)?;
        self.0
            .seal(plain, &recipient)
            .map_err(|_| XkeyExchangeError::CannotSeal)
    }
}

impl fmt::Debug for CalloutXkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CalloutXkey").field(&self.0.public_key()).finish()
    }
}
