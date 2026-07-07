use trogon_std::SecretString;

use super::SecretVerifier;

#[derive(Clone, Debug)]
pub enum SecretMaterial {
    Plaintext(SecretString),
    Verifier(SecretVerifier),
}

impl SecretMaterial {
    pub fn plaintext(value: SecretString) -> Self {
        Self::Plaintext(value)
    }

    pub fn as_plaintext(&self) -> Option<&SecretString> {
        match self {
            Self::Plaintext(value) => Some(value),
            Self::Verifier(_) => None,
        }
    }
}
