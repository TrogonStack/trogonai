use serde::Deserialize;

use crate::peer_certificate_error::PeerCertificateError;

/// The SHA-256 digest of the certificate a TLS peer authenticated with, as 64 lowercase hex digits.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct PeerCertificate(String);

impl TryFrom<String> for PeerCertificate {
    type Error = PeerCertificateError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let hex = value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        (value.len() == 64 && hex)
            .then_some(Self(value))
            .ok_or(PeerCertificateError::NotSha256Hex)
    }
}

#[cfg(test)]
mod tests;
