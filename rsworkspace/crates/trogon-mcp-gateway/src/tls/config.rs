//! PEM loading + rustls server/client config assembly. TLS 1.3 only.
//!
//! Production callers should always go through [`TlsConfig`] so the
//! TLS-version constraint and verifier composition stay
//! consistent across server/client/ingress/egress paths.

use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::ServerConfig;
use rustls::version::TLS13;
use rustls::{ClientConfig, RootCertStore};

use super::verifier::SpiffeClientCertVerifier;

/// PEM-encoded material on disk, plus the SPIFFE/DNS verifier policy.
#[derive(Clone, Debug)]
pub struct TlsConfig {
    pub server_cert_path: PathBuf,
    pub server_key_path: PathBuf,
    pub client_ca_bundle_path: Option<PathBuf>,
    pub spiffe_enabled: bool,
    pub trust_domains: Vec<String>,
    pub allowed_dns_patterns: Vec<String>,
}

/// In-memory parsed view of the on-disk PEMs.
#[derive(Debug)]
pub struct TlsMaterial {
    pub server_chain: Vec<CertificateDer<'static>>,
    pub server_key: PrivateKeyDer<'static>,
    pub client_roots: RootCertStore,
}

#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    #[error("read `{path}`: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse PEM in `{path}`: {source}")]
    Pem {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("`{0}` contains no certificates")]
    NoCerts(PathBuf),
    #[error("`{0}` contains no private key")]
    NoKey(PathBuf),
    #[error("rustls config: {0}")]
    Rustls(#[from] rustls::Error),
    #[error("verifier: {0}")]
    Verifier(String),
}

impl TlsConfig {
    /// Read all PEM material from disk into [`TlsMaterial`].
    pub fn load_material(&self) -> Result<TlsMaterial, TlsConfigError> {
        let server_chain = load_certs(&self.server_cert_path)?;
        let server_key = load_private_key(&self.server_key_path)?;
        let client_roots = match &self.client_ca_bundle_path {
            Some(p) => load_root_store(p)?,
            None => RootCertStore::empty(),
        };
        Ok(TlsMaterial {
            server_chain,
            server_key,
            client_roots,
        })
    }

    /// Build a TLS 1.3-only `ServerConfig` that requires client certs
    /// when a CA bundle was supplied; otherwise serves over TLS
    /// without mTLS (caller's choice).
    pub fn build_server_config(&self) -> Result<Arc<ServerConfig>, TlsConfigError> {
        let material = self.load_material()?;
        let mut builder = ServerConfig::builder_with_protocol_versions(&[&TLS13]);
        let cfg = if self.client_ca_bundle_path.is_some() {
            let verifier = SpiffeClientCertVerifier::new(
                material.client_roots,
                self.spiffe_enabled,
                self.trust_domains.clone(),
                self.allowed_dns_patterns.clone(),
            )
            .map_err(|e| TlsConfigError::Verifier(e.to_string()))?;
            // builder is consumed below.
            let _ = &mut builder;
            ServerConfig::builder_with_protocol_versions(&[&TLS13])
                .with_client_cert_verifier(verifier)
                .with_single_cert(material.server_chain, material.server_key)?
        } else {
            ServerConfig::builder_with_protocol_versions(&[&TLS13])
                .with_no_client_auth()
                .with_single_cert(material.server_chain, material.server_key)?
        };
        Ok(Arc::new(cfg))
    }

    /// TLS 1.3-only `ClientConfig` for outbound mesh connections. The
    /// caller injects the server roots; we don't auto-load any.
    pub fn build_client_config(server_roots: RootCertStore) -> Arc<ClientConfig> {
        let cfg = ClientConfig::builder_with_protocol_versions(&[&TLS13])
            .with_root_certificates(server_roots)
            .with_no_client_auth();
        Arc::new(cfg)
    }
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsConfigError> {
    let bytes = fs::read(path).map_err(|e| TlsConfigError::Read {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut reader = BufReader::new(bytes.as_slice());
    let certs: Result<Vec<_>, _> = rustls_pemfile::certs(&mut reader).collect();
    let certs = certs.map_err(|e| TlsConfigError::Pem {
        path: path.to_path_buf(),
        source: e,
    })?;
    if certs.is_empty() {
        return Err(TlsConfigError::NoCerts(path.to_path_buf()));
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsConfigError> {
    let bytes = fs::read(path).map_err(|e| TlsConfigError::Read {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut reader = BufReader::new(bytes.as_slice());
    let keys: Result<Vec<_>, _> = rustls_pemfile::pkcs8_private_keys(&mut reader).collect();
    let mut keys = keys.map_err(|e| TlsConfigError::Pem {
        path: path.to_path_buf(),
        source: e,
    })?;
    let key = keys.pop().ok_or_else(|| TlsConfigError::NoKey(path.to_path_buf()))?;
    Ok(PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.secret_pkcs8_der().to_vec())))
}

fn load_root_store(path: &Path) -> Result<RootCertStore, TlsConfigError> {
    let certs = load_certs(path)?;
    let mut store = RootCertStore::empty();
    for c in certs {
        store
            .add(c)
            .map_err(|e| TlsConfigError::Verifier(e.to_string()))?;
    }
    Ok(store)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair};
    use tempfile::tempdir;

    fn write_server_pems(dir: &Path) -> (PathBuf, PathBuf) {
        let kp = KeyPair::generate().unwrap();
        let cert = CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .self_signed(&kp)
            .unwrap();
        let cert_path = dir.join("server.pem");
        let key_path = dir.join("server.key.pem");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, kp.serialize_pem()).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn loads_material_round_trip() {
        let dir = tempdir().unwrap();
        let (cert, key) = write_server_pems(dir.path());
        let cfg = TlsConfig {
            server_cert_path: cert,
            server_key_path: key,
            client_ca_bundle_path: None,
            spiffe_enabled: false,
            trust_domains: vec![],
            allowed_dns_patterns: vec![],
        };
        let mat = cfg.load_material().unwrap();
        assert_eq!(mat.server_chain.len(), 1);
    }

    #[test]
    fn build_server_config_without_mtls_succeeds() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let dir = tempdir().unwrap();
        let (cert, key) = write_server_pems(dir.path());
        let cfg = TlsConfig {
            server_cert_path: cert,
            server_key_path: key,
            client_ca_bundle_path: None,
            spiffe_enabled: false,
            trust_domains: vec![],
            allowed_dns_patterns: vec![],
        };
        cfg.build_server_config().expect("server config");
    }

    #[test]
    fn missing_cert_file_surfaces_read_error() {
        let cfg = TlsConfig {
            server_cert_path: PathBuf::from("/nonexistent.pem"),
            server_key_path: PathBuf::from("/nonexistent.key.pem"),
            client_ca_bundle_path: None,
            spiffe_enabled: false,
            trust_domains: vec![],
            allowed_dns_patterns: vec![],
        };
        let err = cfg.load_material().unwrap_err();
        assert!(matches!(err, TlsConfigError::Read { .. }));
    }
}
