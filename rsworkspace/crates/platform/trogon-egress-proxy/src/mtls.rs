//! The static-file mutual-TLS listener configuration (ADR#0085 Decision 3).
//!
//! This is the legacy credential path: the proxy and its clients hold private
//! keys on disk, and it is to be retired once the Workload API path is deployed.
//! Its certificate profile is deferred by ADR#0085 Decision 3, but the SVID
//! lifetime ceiling is not: a certificate valid for more than one hour
//! ([`crate::svid_lifetime`]) is refused, so a long-lived static certificate
//! cannot be presented. Keys still sit on disk and rotation is manual.
//!
//! A client must present a certificate that chains to the configured trust
//! anchors, that satisfies the X.509-SVID key constraints, and that names
//! exactly one agent in its single URI SAN. The constraints are
//! [`trogon_workload_identity::verify_leaf_key_profile`], the same check the
//! Workload API path applies: `cA` false, `digitalSignature` set, neither
//! `keyCertSign` nor `cRLSign`, and, when an extended key usage is present, both
//! `serverAuth` and `clientAuth`. A client certificate with only `clientAuth` is
//! therefore refused, as it is on the Workload API path. All of it is decided
//! inside the handshake, so a client that fails any check never gets to send an
//! HTTP byte.
//!
//! The same key constraints and lifetime ceiling hold for the certificate the
//! proxy itself presents. It is checked when the configuration is built, and
//! startup fails with a [`TlsError`] rather than serving a non-conforming
//! identity. A static certificate issued for longer than one hour must be
//! reissued.

use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, OtherError, RootCertStore, ServerConfig,
    SignatureScheme,
};

use trogon_workload_identity::{MaterialError, verify_leaf_key_profile};
use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer;

use crate::agent_id::AgentId;
use crate::constants::{CLAIMED_URI_MAX_LEN, SPIFFE_SCHEME_PREFIX};
use crate::peer_certificate::{PeerIdentityError, agent_id_from_certificate};
use crate::svid_lifetime::{SvidLifetimeError, verify_svid_lifetime};
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
    #[error("a client trust anchor in {path} names {uri}, not the trust domain {trust_domain}")]
    ForeignTrustAnchor {
        path: PathBuf,
        uri: String,
        trust_domain: TrustDomain,
    },
    #[error("a client trust anchor in {path} carries more than one URI subject alternative name")]
    AmbiguousTrustAnchor { path: PathBuf },
    #[error("server certificate breaks the SVID key profile")]
    ServerCertificateProfile(#[source] MaterialError),
    #[error("server certificate exceeds the X.509-SVID lifetime ceiling")]
    ServerCertificateLifetime(#[source] SvidLifetimeError),
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
        verify_anchor_trust_domain(&anchor, anchors_path, &trust_domain)?;
        roots.add(anchor).map_err(|source| TlsError::InvalidTrustAnchor {
            path: anchors_path.to_owned(),
            source,
        })?;
    }
    let inner = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
        .build()
        .map_err(TlsError::Verifier)?;
    let leaf = chain
        .first()
        .ok_or(TlsError::Configuration(rustls::Error::NoCertificatesPresented))?;
    verify_leaf_key_profile(leaf.as_ref()).map_err(TlsError::ServerCertificateProfile)?;
    verify_svid_lifetime(leaf.as_ref()).map_err(TlsError::ServerCertificateLifetime)?;
    let verifier = SingleAgentVerifier { inner, trust_domain };
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(TlsError::Configuration)?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_single_cert(chain, key)
        .map_err(TlsError::Configuration)?;
    // Resumption bypasses client certificate verification on a new connection.
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Ok(config)
}

/// All anchors of one file share a single root store, and the leaf's URI SAN
/// is the only trust domain check, so an anchor that belongs to another trust
/// domain would let its CA mint leaves for this one. An anchor that names a
/// trust domain, as a SPIRE CA certificate does, must name exactly this one. An
/// anchor with no URI SAN, such as an upstream root, is not judged here.
fn verify_anchor_trust_domain(
    anchor: &CertificateDer<'_>,
    anchors_path: &Path,
    trust_domain: &TrustDomain,
) -> Result<(), TlsError> {
    let unusable = || TlsError::InvalidTrustAnchor {
        path: anchors_path.to_owned(),
        source: rustls::Error::InvalidCertificate(CertificateError::BadEncoding),
    };
    let (_, certificate) = X509Certificate::from_der(anchor.as_ref())
        .ok()
        .filter(|(rest, _)| rest.is_empty())
        .ok_or_else(unusable)?;
    let san = certificate.subject_alternative_name().map_err(|_| unusable())?;
    let mut uris = san
        .into_iter()
        .flat_map(|san| san.value.general_names.iter())
        .filter_map(|name| match name {
            GeneralName::URI(uri) => Some(*uri),
            _ => None,
        });
    let Some(uri) = uris.next() else {
        return Ok(());
    };
    if uris.next().is_some() {
        return Err(TlsError::AmbiguousTrustAnchor {
            path: anchors_path.to_owned(),
        });
    }
    if uri.strip_prefix(SPIFFE_SCHEME_PREFIX) != Some(trust_domain.as_str()) {
        return Err(TlsError::ForeignTrustAnchor {
            path: anchors_path.to_owned(),
            uri: uri.chars().take(CLAIMED_URI_MAX_LEN).collect(),
            trust_domain: trust_domain.clone(),
        });
    }
    Ok(())
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
/// breaks the X.509-SVID key constraints, is valid for longer than the SVID
/// lifetime ceiling, or does not name exactly one agent in this trust domain,
/// is refused in the handshake.
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
        verify_leaf_key_profile(end_entity.as_ref()).map_err(|error| {
            rustls::Error::General(format!("client certificate breaks the SVID key profile: {error}"))
        })?;
        verify_svid_lifetime(end_entity.as_ref())
            .map_err(|error| rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(error)))))?;
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
