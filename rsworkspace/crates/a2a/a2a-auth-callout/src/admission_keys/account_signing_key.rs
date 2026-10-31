use std::fmt;

use nkeys::{KeyPair, KeyPairType};

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AccountSigningKeyError {
    #[error("is not an NKey seed")]
    NotASeed,
    #[error("is an NKey seed of something other than an account")]
    NotAnAccountSeed,
}

#[derive(Clone)]
pub struct AccountSigningKey(KeyPair);

impl AccountSigningKey {
    pub fn from_seed(seed: &str) -> Result<Self, AccountSigningKeyError> {
        let key = KeyPair::from_seed(seed.trim()).map_err(|_| AccountSigningKeyError::NotASeed)?;
        if key.key_pair_type() != KeyPairType::Account {
            return Err(AccountSigningKeyError::NotAnAccountSeed);
        }
        Ok(Self(key))
    }

    pub fn public_key(&self) -> String {
        self.0.public_key()
    }

    pub(crate) fn keypair(&self) -> &KeyPair {
        &self.0
    }
}

impl fmt::Debug for AccountSigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AccountSigningKey").field(&self.0.public_key()).finish()
    }
}
