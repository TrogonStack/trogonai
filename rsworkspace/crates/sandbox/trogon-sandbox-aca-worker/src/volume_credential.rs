use trogon_sandbox_contract::ShortLivedToken;

use crate::AcaError;

/// The storage account credential a volume token carries.
#[derive(Debug, Clone)]
pub struct VolumeCredential {
    pub account: String,
    pub key: ShortLivedToken,
}

impl VolumeCredential {
    /// Reads a token of the form `account:key[:session_token]`.
    pub fn parse(token: &ShortLivedToken) -> Result<Self, AcaError> {
        let mut parts = token.expose_secret().split(':');
        match (parts.next(), parts.next()) {
            (Some(account), Some(key)) => Ok(Self {
                account: account.to_owned(),
                key: ShortLivedToken::new(key),
            }),
            _ => Err(AcaError::VolumeCredentialFormat),
        }
    }
}
