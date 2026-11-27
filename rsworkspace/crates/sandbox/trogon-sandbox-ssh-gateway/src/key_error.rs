/// A session key could not be generated.
#[derive(Debug, thiserror::Error)]
#[error("generate ed25519 key: {0}")]
pub struct KeyError(#[from] russh::keys::ssh_key::Error);
