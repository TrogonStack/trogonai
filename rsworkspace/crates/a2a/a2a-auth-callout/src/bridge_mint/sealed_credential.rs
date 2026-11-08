use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nkeys::XKey;
use serde::{Deserialize, Serialize};

use crate::admission_keys::{CalloutXkey, XkeyExchangeError, XkeyPublic};
use crate::credentials::PresentedCredential;
use crate::credentials::api_key::ApiKey;
use crate::credentials::oidc::BearerToken;

/// The external credential the edge forwards so the callout can verify the
/// caller itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeCredential {
    BearerToken(BearerToken),
    ApiKey(ApiKey),
}

impl BridgeCredential {
    fn secret(&self) -> &str {
        match self {
            Self::BearerToken(token) => token.as_str(),
            Self::ApiKey(key) => key.as_str(),
        }
    }
}

impl From<BridgeCredential> for PresentedCredential {
    fn from(credential: BridgeCredential) -> Self {
        match credential {
            BridgeCredential::BearerToken(token) => Self::BearerToken(token),
            BridgeCredential::ApiKey(key) => Self::ApiKey(key),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeCredentialError {
    #[error("no callout xkey is configured, so no caller credential can cross the mint exchange")]
    NoCalloutXkey,
    #[error("the callout xkey is not a curve public key")]
    InvalidCalloutXkey,
    #[error("the caller credential is blank")]
    BlankSecret,
    #[error("the caller credential cannot be encoded for sealing")]
    Encode(#[source] serde_json::Error),
    #[error("the caller credential cannot be sealed to the callout xkey")]
    CannotSeal,
    #[error("the sealed caller credential does not open with the callout xkey")]
    CannotOpen(#[source] XkeyExchangeError),
    #[error("the sealed caller credential is not a credential document")]
    Malformed(#[source] serde_json::Error),
}

/// The document sealed inside a [`SealedBridgeCredential`].
#[derive(Serialize, Deserialize)]
#[serde(tag = "scheme", rename_all = "snake_case", deny_unknown_fields)]
enum SealedPlaintext {
    BearerToken { token: String },
    ApiKey { key: String },
}

impl SealedPlaintext {
    fn of(credential: &BridgeCredential) -> Self {
        match credential {
            BridgeCredential::BearerToken(token) => Self::BearerToken {
                token: token.as_str().to_owned(),
            },
            BridgeCredential::ApiKey(key) => Self::ApiKey {
                key: key.as_str().to_owned(),
            },
        }
    }

    fn into_credential(self) -> Result<BridgeCredential, BridgeCredentialError> {
        let credential = match self {
            Self::BearerToken { token } => BridgeCredential::BearerToken(BearerToken::new(token)),
            Self::ApiKey { key } => {
                BridgeCredential::ApiKey(ApiKey::new(key).map_err(|_| BridgeCredentialError::BlankSecret)?)
            }
        };
        refuse_blank(&credential)?;
        Ok(credential)
    }
}

fn refuse_blank(credential: &BridgeCredential) -> Result<(), BridgeCredentialError> {
    if credential.secret().trim().is_empty() {
        return Err(BridgeCredentialError::BlankSecret);
    }
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
struct SealedBytes(Vec<u8>);

impl TryFrom<String> for SealedBytes {
    type Error = base64::DecodeError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        URL_SAFE_NO_PAD.decode(value).map(Self)
    }
}

impl From<SealedBytes> for String {
    fn from(value: SealedBytes) -> Self {
        URL_SAFE_NO_PAD.encode(value.0)
    }
}

/// A [`BridgeCredential`] sealed to the callout xkey from a one-time sender
/// key, so only the callout reads it and no two exchanges share a sender.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedBridgeCredential {
    sender: XkeyPublic,
    sealed: SealedBytes,
}

impl SealedBridgeCredential {
    pub fn seal(credential: &BridgeCredential, callout: &XkeyPublic) -> Result<Self, BridgeCredentialError> {
        refuse_blank(credential)?;
        let recipient = callout
            .to_xkey()
            .map_err(|_| BridgeCredentialError::InvalidCalloutXkey)?;
        let plaintext = serde_json::to_vec(&SealedPlaintext::of(credential)).map_err(BridgeCredentialError::Encode)?;
        let sender = XKey::new();
        let sealed = sender
            .seal(&plaintext, &recipient)
            .map_err(|_| BridgeCredentialError::CannotSeal)?;
        Ok(Self {
            sender: XkeyPublic::from_xkey(&sender),
            sealed: SealedBytes(sealed),
        })
    }

    pub fn open(&self, callout: &CalloutXkey) -> Result<BridgeCredential, BridgeCredentialError> {
        let plaintext = callout
            .open(&self.sealed.0, &self.sender)
            .map_err(BridgeCredentialError::CannotOpen)?;
        serde_json::from_slice::<SealedPlaintext>(&plaintext)
            .map_err(BridgeCredentialError::Malformed)?
            .into_credential()
    }
}

impl fmt::Debug for SealedBridgeCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SealedBridgeCredential")
            .field("sender", &self.sender)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
