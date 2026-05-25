mod env;
mod file;
mod key_version;
mod loader;
mod minting_material;
mod signing_key_handle;
mod static_source;
mod vault;

pub use env::EnvSigningKeySource;
pub use file::FileSigningKeySource;
pub use loader::signing_key_source_from_process_env;
pub use key_version::{KeyVersion, KeyVersionError};
pub(crate) use key_version::unminted_placeholder;
pub use minting_material::MintingMaterial;
pub use signing_key_handle::SigningKeyHandle;
pub use static_source::StaticSigningKeySource;
pub use vault::VaultSigningKeySource;

pub trait SigningKeySource: Send + Sync {
    fn current(&self) -> SigningKeyHandle;
    fn accepted(&self) -> Vec<SigningKeyHandle>;
}

#[cfg(test)]
mod tests;
