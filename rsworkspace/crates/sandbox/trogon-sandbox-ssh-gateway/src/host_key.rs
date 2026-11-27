use std::path::Path;

use russh::keys::{PrivateKey, decode_secret_key, encode_pkcs8_pem};
use tracing::info;

use crate::HostKeyError;
pub use crate::constants::DEFAULT_HOST_KEY_PATH;
use crate::ephemeral_session_key::generate_ed25519;

/// Reads the PEM host key at `path`, or generates one and writes it there when the file does not exist.
pub fn load_or_generate_host_key(path: &Path) -> Result<PrivateKey, HostKeyError> {
    let owned = || path.to_path_buf();
    if path.exists() {
        let pem = std::fs::read_to_string(path).map_err(|source| HostKeyError::Read { path: owned(), source })?;
        let key = decode_secret_key(&pem, None).map_err(|source| HostKeyError::Decode { path: owned(), source })?;
        info!(path = %path.display(), "loaded SSH host key");
        return Ok(key);
    }
    let parent = path.parent().unwrap_or(Path::new(""));
    std::fs::create_dir_all(parent).map_err(|source| HostKeyError::CreateDirectory {
        path: parent.to_path_buf(),
        source,
    })?;
    let key = generate_ed25519()?;
    let mut pem = Vec::new();
    encode_pkcs8_pem(&key, &mut pem)?;
    std::fs::write(path, pem).map_err(|source| HostKeyError::Write { path: owned(), source })?;
    info!(path = %path.display(), "generated new SSH host key");
    Ok(key)
}
