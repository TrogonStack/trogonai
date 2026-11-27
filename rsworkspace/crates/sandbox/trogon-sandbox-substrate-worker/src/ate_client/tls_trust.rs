use std::fs;
use std::path::Path;

use tonic::transport::{Certificate, ClientTlsConfig};

/// Trust settings for the TLS connection to `ate-api-server`.
///
/// The server presents a certificate signed by Substrate's own pod-cert issuer, not a
/// public CA, so the client has to be told which CA to trust explicitly. `server_name`
/// overrides the name checked against that certificate independently of the address
/// dialed, for cases like a port-forward to `localhost`; when unset it falls back to
/// the endpoint's own host.
#[derive(Debug, Clone)]
pub struct TlsTrust {
    ca_certificate_pem: Vec<u8>,
    server_name: Option<String>,
}

impl TlsTrust {
    pub fn from_ca_file(path: impl AsRef<Path>, server_name: Option<String>) -> std::io::Result<Self> {
        Ok(Self {
            ca_certificate_pem: fs::read(path)?,
            server_name,
        })
    }

    pub(super) fn into_client_tls_config(self) -> ClientTlsConfig {
        let config = ClientTlsConfig::new().ca_certificate(Certificate::from_pem(self.ca_certificate_pem));
        match self.server_name {
            Some(server_name) => config.domain_name(server_name),
            None => config,
        }
    }
}
