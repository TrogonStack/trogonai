use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use super::RouterTlsError;
use super::pem::{certificates, read_pem};

#[derive(Clone, Debug)]
pub struct RouterTlsConfig {
    ca_path: PathBuf,
    client_cert_path: PathBuf,
    client_key_path: PathBuf,
    server_name: ServerName<'static>,
}

impl RouterTlsConfig {
    pub fn new(
        ca_path: impl Into<PathBuf>,
        client_cert_path: impl Into<PathBuf>,
        client_key_path: impl Into<PathBuf>,
        server_name: impl Into<String>,
    ) -> Result<Self, RouterTlsError> {
        let ca_path = ca_path.into();
        let client_cert_path = client_cert_path.into();
        let client_key_path = client_key_path.into();
        if [&ca_path, &client_cert_path, &client_key_path]
            .iter()
            .any(|path| path.as_os_str().is_empty())
        {
            return Err(RouterTlsError::MissingPath);
        }
        let server_name = ServerName::try_from(server_name.into()).map_err(|_| RouterTlsError::InvalidServerName)?;
        Ok(Self {
            ca_path,
            client_cert_path,
            client_key_path,
            server_name,
        })
    }

    pub(crate) async fn connect(&self, address: &str) -> Result<TlsStream<TcpStream>, RouterTlsError> {
        let paths = self.clone();
        let config = tokio::task::spawn_blocking(move || paths.load())
            .await
            .map_err(RouterTlsError::LoadTask)??;
        let socket = TcpStream::connect(address).await.map_err(RouterTlsError::Connect)?;
        TlsConnector::from(Arc::new(config))
            .connect(self.server_name.clone(), socket)
            .await
            .map_err(RouterTlsError::Handshake)
    }

    fn load(&self) -> Result<ClientConfig, RouterTlsError> {
        let ca_pem = read_pem(&self.ca_path, "CA trust bundle")?;
        let ca_certs = certificates(&ca_pem, "CA trust bundle")?;
        let mut roots = RootCertStore::empty();
        for cert in ca_certs {
            roots.add(cert).map_err(|_| RouterTlsError::InvalidCaCertificate)?;
        }

        let cert_pem = read_pem(&self.client_cert_path, "client certificate bundle")?;
        let certs = certificates(&cert_pem, "client certificate bundle")?;
        // Kubernetes projects the private key and certificate chain in one atomically
        // rotated file. Reading that shared path once keeps the identity coherent.
        let separate_key_pem = if self.client_key_path == self.client_cert_path {
            None
        } else {
            Some(read_pem(&self.client_key_path, "client private key")?)
        };
        let key_pem = separate_key_pem.as_deref().unwrap_or(&cert_pem);
        let key = rustls_pemfile::private_key(&mut Cursor::new(key_pem))
            .map_err(|_| RouterTlsError::InvalidKeyPem)?
            .ok_or(RouterTlsError::MissingPrivateKey)?;

        ClientConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::aws_lc_rs::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(RouterTlsError::ProtocolVersions)?
            .with_root_certificates(roots)
            .with_client_auth_cert(certs, key)
            .map_err(|_| RouterTlsError::InvalidClientIdentity)
    }
}

#[cfg(test)]
mod tests;
