use std::fmt;

use sha2::{Digest, Sha256};

use super::{InstallationId, NativeSessionId, TrustedPeerOwner};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerId(String);

impl PeerId {
    pub fn parse(value: impl Into<String>) -> Result<Self, super::PeerValueError> {
        Ok(Self(value.into()))
    }

    pub(crate) fn derive(
        owner: &TrustedPeerOwner,
        installation: &InstallationId,
        native_session: &NativeSessionId,
    ) -> Self {
        let mut digest = Sha256::new();
        for value in [
            "trogon/peers/enrollment/v1",
            owner.tenant().as_str(),
            owner.caller().as_str(),
            installation.as_str(),
            native_session.as_str(),
        ] {
            digest.update((value.len() as u32).to_be_bytes());
            digest.update(value.as_bytes());
        }
        Self(hex::encode(digest.finalize()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
