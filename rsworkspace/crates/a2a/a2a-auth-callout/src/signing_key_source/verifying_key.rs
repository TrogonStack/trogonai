use nkeys::KeyPair;

use super::key_version::KeyVersion;
use super::signing_key_handle::SigningKeyHandle;
use crate::admission_keys::IssuerPublicKey;
use crate::jwt::JwtError;

/// The public half of an issuer key: everything verifying a minted user JWT
/// needs, and nothing that could mint one.
#[derive(Debug, Clone)]
pub struct VerifyingKey {
    version: Option<KeyVersion>,
    issuer_public: String,
}

impl VerifyingKey {
    /// A key whose version is unknown, so it is tried for every token rather
    /// than selected by the token's `kid`.
    pub fn from_issuer(issuer: &IssuerPublicKey) -> Self {
        Self {
            version: None,
            issuer_public: issuer.as_str().to_owned(),
        }
    }

    pub(super) fn from_handle(handle: &SigningKeyHandle) -> Self {
        Self {
            version: Some(handle.version().clone()),
            issuer_public: handle.minting_material().issuer_public(),
        }
    }

    pub fn version(&self) -> Option<&KeyVersion> {
        self.version.as_ref()
    }

    pub fn issuer_public(&self) -> &str {
        &self.issuer_public
    }

    pub(crate) fn verify(&self, input: &[u8], signature: &[u8]) -> Result<(), JwtError> {
        KeyPair::from_public_key(&self.issuer_public)
            .and_then(|issuer| issuer.verify(input, signature))
            .map_err(|e| JwtError::Decode(e.to_string()))
    }
}

/// The issuer keys a verifier accepts.
///
/// Every [`super::SigningKeySource`] provides this, so the callout verifies
/// against the keys it mints with. A process that only checks tokens holds an
/// [`super::IssuerPublicKeys`] instead and never loads a seed.
pub trait VerifyingKeySource: Send + Sync {
    fn accepted_verifying_keys(&self) -> Vec<VerifyingKey>;
}

impl<T: super::SigningKeySource + ?Sized> VerifyingKeySource for T {
    fn accepted_verifying_keys(&self) -> Vec<VerifyingKey> {
        self.accepted().iter().map(VerifyingKey::from_handle).collect()
    }
}
