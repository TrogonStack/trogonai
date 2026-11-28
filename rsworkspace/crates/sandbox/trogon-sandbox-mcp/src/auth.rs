//! Where the API key comes from: the command line first, then a private file under the user's config directory.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use trogon_std::env::ReadEnv;

use crate::auth_error::AuthError;
use crate::constants::{API_KEY_FILE_MODE, API_KEY_PATH_SEGMENTS};

/// The key given on the command line, else the one in the user's key file.
///
/// No key file, or no home directory to look in, is no key. A key file that exists but is unreadable, readable by
/// others or empty is an error rather than a silent absence, so a misconfigured key is never mistaken for none.
pub fn resolve_api_key(cli_key: Option<String>, env: &impl ReadEnv) -> Result<Option<String>, AuthError> {
    if let Some(key) = cli_key.filter(|k| !k.is_empty()) {
        return Ok(Some(key));
    }
    let Some(path) = default_api_key_path(env) else {
        return Ok(None);
    };
    match read_api_key_file(&path) {
        Ok(key) => Ok(Some(key)),
        Err(AuthError::NotAFile { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// `$HOME` (or `$USERPROFILE`) joined with the key file's relative path, or `None` when neither is set.
pub fn default_api_key_path(env: &impl ReadEnv) -> Option<PathBuf> {
    let home = env.var("HOME").or_else(|_| env.var("USERPROFILE")).ok()?;
    Some(
        API_KEY_PATH_SEGMENTS
            .iter()
            .fold(PathBuf::from(home), |path, segment| path.join(segment)),
    )
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
