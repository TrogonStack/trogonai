use russh::keys::{Algorithm, PrivateKey, PublicKeyBase64};

use crate::KeyError;

/// An Ed25519 key pair that lives for one gateway session.
pub struct EphemeralSessionKey {
    pub key_pair: PrivateKey,
    /// The OpenSSH authorized_keys line for the public half.
    pub public_key_line: String,
}

impl EphemeralSessionKey {
    pub fn generate() -> Result<Self, KeyError> {
        let key_pair = generate_ed25519()?;
        let public_key = key_pair.public_key();
        let public_key_line = format!(
            "{} {} sandbox-session",
            public_key.algorithm().as_str(),
            public_key.public_key_base64()
        );
        Ok(Self {
            key_pair,
            public_key_line,
        })
    }
}

pub(crate) fn generate_ed25519() -> Result<PrivateKey, KeyError> {
    let mut rng = russh::keys::key::safe_rng();
    Ok(PrivateKey::random(&mut rng, Algorithm::Ed25519)?)
}
