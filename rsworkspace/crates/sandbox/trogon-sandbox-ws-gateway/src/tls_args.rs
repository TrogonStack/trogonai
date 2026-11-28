use std::path::PathBuf;

use clap::Args;

use crate::TlsFiles;

/// The `--tls-cert` and `--tls-key` flags. Each one requires the other, and [`TlsArgs::files`] turns a complete pair
/// into a [`TlsFiles`].
#[derive(Args, Debug, Clone, Default)]
pub struct TlsArgs {
    /// PEM file with the certificate chain. Requires `--tls-key`.
    #[arg(long = "tls-cert", env = "SANDBOX_WS_TLS_CERT", requires = "tls_key")]
    tls_cert: Option<PathBuf>,

    /// PEM file with the private key. Requires `--tls-cert`.
    #[arg(long = "tls-key", env = "SANDBOX_WS_TLS_KEY", requires = "tls_cert")]
    tls_key: Option<PathBuf>,
}

impl TlsArgs {
    pub fn files(&self) -> Option<TlsFiles> {
        self.tls_cert
            .clone()
            .zip(self.tls_key.clone())
            .map(|(cert, key)| TlsFiles::new(cert, key))
    }
}
