mod env;
mod file;
mod key_version;
mod signing_key_handle;
mod static_source;
mod vault;

pub use env::EnvSigningKeySource;
pub use file::FileSigningKeySource;
pub use key_version::{KeyVersion, KeyVersionError};
pub(crate) use key_version::unminted_placeholder;
pub use signing_key_handle::SigningKeyHandle;
pub use static_source::StaticSigningKeySource;
pub use vault::VaultSigningKeySource;

pub trait SigningKeySource: Send + Sync {
    fn current(&self) -> SigningKeyHandle;
    fn accepted(&self) -> Vec<SigningKeyHandle>;
}

#[cfg(test)]
mod tests;
