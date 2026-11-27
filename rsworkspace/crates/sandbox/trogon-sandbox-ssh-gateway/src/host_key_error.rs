use std::io;
use std::path::PathBuf;

use crate::KeyError;

/// The gateway's host key could not be loaded or created.
#[derive(Debug, thiserror::Error)]
pub enum HostKeyError {
    #[error("read host key {path}: {source}")]
    Read { path: PathBuf, source: io::Error },
    #[error("decode host key {path}: {source}")]
    Decode { path: PathBuf, source: russh::keys::Error },
    #[error("create host key directory {path}: {source}")]
    CreateDirectory { path: PathBuf, source: io::Error },
    #[error("generate host key: {0}")]
    Generate(#[from] KeyError),
    #[error("encode generated host key: {0}")]
    Encode(#[from] russh::keys::Error),
    #[error("write host key {path}: {source}")]
    Write { path: PathBuf, source: io::Error },
}
