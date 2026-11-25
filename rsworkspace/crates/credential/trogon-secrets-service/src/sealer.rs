use trogon_std::SecretString;

use super::{RequesterXkey, SealError, SealedValue};

/// Seals a credential value to the requester's xkey.
pub trait Sealer: Send + Sync + 'static {
    fn seal(&self, value: &SecretString, recipient: &RequesterXkey) -> Result<SealedValue, SealError>;
}
