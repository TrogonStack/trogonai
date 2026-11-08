//! The mutual-TLS listener configuration (ADR#0085 Decision 3).
//!
//! A client must present a certificate that chains to the configured trust
//! anchors, and that certificate must name exactly one agent in its single URI
//! SAN. Both are decided inside the handshake, so a client that fails either
//! never gets to send an HTTP byte.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme};

use crate::agent_id::AgentId;
use crate::peer_certificate::{PeerIdentityError, agent_id_from_certificate};
use crate::trust_domain::TrustDomain;

/// Where the listener's TLS material lives on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsFiles {
    /// PEM certificate chain the proxy presents, leaf first.
    pub certificate_chain: PathBuf,
    pub private_key: PathBuf,
    /// PEM bundle of the trust anchors client certificates must chain to.
    pub client_trust_anchors: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum TlsError {
    #[error("cannot read {path}")]
    Unreadable {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} holds no certificate")]
    NoCertificate { path: PathBuf },
    #[error("{path} holds no private key")]
    NoPrivateKey { path: PathBuf },
    #[error("a client trust anchor in {path} is not usable")]
    InvalidTrustAnchor {
        path: PathBuf,
        #[source]
        source: rustls::Error,
    },
    #[error("client certificate verifier could not be built")]
    Verifier(#[source] rustls::server::VerifierBuilderError),
    #[error("server certificate and key do not form a valid TLS configuration")]
    Configuration(#[source] rustls::Error),
}

/// Reads the files and builds the listener configuration.
pub fn load_server_config(files: &TlsFiles, trust_domain: TrustDomain) -> Result<ServerConfig, TlsError> {
    let chain = read_certificates(&files.certificate_chain)?;
    let key = read_private_key(&files.private_key)?;
    let anchors = read_certificates(&files.client_trust_anchors)?;
    server_config(chain, key, anchors, &files.client_trust_anchors, trust_domain)
}

/// `anchors_path` is only used to name the file in errors.
pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    anchors: Vec<CertificateDer<'static>>,
    anchors_path: &Path,
    trust_domain: TrustDomain,
) -> Result<ServerConfig, TlsError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = RootCertStore::empty();
    for anchor in anchors {
        roots.add(anchor).map_err(|source| TlsError::InvalidTrustAnchor {
            path: anchors_path.to_owned(),
            source,
        })?;
    }
    let inner = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
        .build()
        .map_err(TlsError::Verifier)?;
    let verifier = SingleAgentVerifier { inner, trust_domain };
    ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(TlsError::Configuration)?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(chain, key)
        .map_err(TlsError::Configuration)
}

/// The caller of an established connection: the single URI SAN of the leaf
/// the handshake verified, mapped to an [`AgentId`].
pub fn peer_agent(
    peer_certificates: Option<&[CertificateDer<'_>]>,
    trust_domain: &TrustDomain,
) -> Result<AgentId, PeerIdentityError> {
    let leaf = peer_certificates
        .and_then(<[_]>::first)
        .ok_or(PeerIdentityError::NoUriSan)?;
    agent_id_from_certificate(leaf.as_ref(), trust_domain)
}

fn read_certificates(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let mut reader = open(path)?;
    let certificates = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| TlsError::Unreadable {
            path: path.to_owned(),
            source,
        })?;
    if certificates.is_empty() {
        return Err(TlsError::NoCertificate { path: path.to_owned() });
    }
    Ok(certificates)
}

fn read_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    let mut reader = open(path)?;
    rustls_pemfile::private_key(&mut reader)
        .map_err(|source| TlsError::Unreadable {
            path: path.to_owned(),
            source,
        })?
        .ok_or_else(|| TlsError::NoPrivateKey { path: path.to_owned() })
}

fn open(path: &Path) -> Result<BufReader<File>, TlsError> {
    File::open(path)
        .map(BufReader::new)
        .map_err(|source| TlsError::Unreadable {
            path: path.to_owned(),
            source,
        })
}

/// Chain validation is the stock WebPKI verifier's. On top of it, a leaf that
/// does not name exactly one agent in this trust domain is refused in the
/// handshake.
#[derive(Debug)]
struct SingleAgentVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    trust_domain: TrustDomain,
}

impl ClientCertVerifier for SingleAgentVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let verified = self.inner.verify_client_cert(end_entity, intermediates, now)?;
        agent_id_from_certificate(end_entity.as_ref(), &self.trust_domain)
            .map_err(|error| rustls::Error::General(format!("client certificate names no single agent: {error}")))?;
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }

    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
