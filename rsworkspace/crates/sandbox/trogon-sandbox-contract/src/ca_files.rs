use std::path::{Path, PathBuf};

/// The CA certificate and private key files the proxy signs intercepted connections with instead of a generated
/// one. Both are given or neither.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct CaFiles {
    cert_path: PathBuf,
    key_path: PathBuf,
}

impl CaFiles {
    pub fn new(cert_path: PathBuf, key_path: PathBuf) -> Self {
        Self { cert_path, key_path }
    }

    pub fn cert_path(&self) -> &Path {
        &self.cert_path
    }

    pub fn key_path(&self) -> &Path {
        &self.key_path
    }
}
