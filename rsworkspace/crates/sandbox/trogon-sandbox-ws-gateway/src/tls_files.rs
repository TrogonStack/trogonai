use std::path::{Path, PathBuf};

/// PEM certificate chain and private key the gateway terminates TLS with.
///
/// The two files only exist together, so a lone certificate or a lone key cannot be represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    cert: PathBuf,
    key: PathBuf,
}

impl TlsFiles {
    pub fn new(cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Self {
        Self {
            cert: cert.into(),
            key: key.into(),
        }
    }

    pub fn cert(&self) -> &Path {
        &self.cert
    }

    pub fn key(&self) -> &Path {
        &self.key
    }
}
