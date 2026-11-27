//! Where the API key comes from: the command line first, then a private file under the user's config directory.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use trogon_std::env::ReadEnv;

use crate::auth_error::AuthError;
use crate::constants::{API_KEY_FILE_MODE, API_KEY_PATH_SEGMENTS};

/// The key given on the command line, else the one in the user's key file. A missing or unusable file is no key.
pub fn resolve_api_key(cli_key: Option<String>, env: &impl ReadEnv) -> Option<String> {
    if let Some(key) = cli_key.filter(|k| !k.is_empty()) {
        return Some(key);
    }
    read_api_key_file(&default_api_key_path(env)).ok()
}

/// `$HOME` (or `$USERPROFILE`, or the working directory) joined with the key file's relative path.
pub fn default_api_key_path(env: &impl ReadEnv) -> PathBuf {
    let home = env
        .var("HOME")
        .or_else(|_| env.var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    API_KEY_PATH_SEGMENTS
        .iter()
        .fold(PathBuf::from(home), |path, segment| path.join(segment))
}

/// Reads a key from `path`, which must be a file only its owner can touch.
pub fn read_api_key_file(path: &Path) -> Result<String, AuthError> {
    let read_error = |source| AuthError::ReadFile {
        path: path.to_path_buf(),
        source,
    };
    if !path.is_file() {
        return Err(AuthError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    let mode = std::fs::metadata(path).map_err(read_error)?.permissions().mode() & 0o777;
    if mode != API_KEY_FILE_MODE {
        return Err(AuthError::InsecureFileMode {
            path: path.to_path_buf(),
            mode,
        });
    }
    let contents = std::fs::read_to_string(path).map_err(read_error)?;
    let key = contents.trim();
    if key.is_empty() {
        return Err(AuthError::Empty {
            path: path.to_path_buf(),
        });
    }
    Ok(key.to_string())
}

#[cfg(test)]
mod tests;
