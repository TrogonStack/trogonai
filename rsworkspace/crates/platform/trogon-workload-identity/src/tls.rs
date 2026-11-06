use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme};
use spiffe::SpiffeId;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::candidate::{Candidate, leaf_profile, timestamp};
use crate::identity_store::MaterialAuthority;
use crate::tls_guards::LiveResolver;
use crate::{MaterialError, RegistrationSet};

pub(crate) fn verify_chain(
    chain: &[CertificateDer<'_>],
    roots: &[CertificateDer<'_>],
    now: SystemTime,
    server: bool,
) -> Result<SystemTime, MaterialError> {
    let leaf = chain.first().ok_or(MaterialError::Certificate)?;
    let (_, _, leaf_expiry) = leaf_profile(leaf, now)?;
    let mut usable = Vec::new();
    let mut expiries = Vec::new();
    for root in roots {
        let (rest, certificate) = X509Certificate::from_der(root).map_err(|_| MaterialError::Certificate)?;
        if !rest.is_empty() {
            return Err(MaterialError::Certificate);
        }
        let not_before = timestamp(certificate.validity().not_before.timestamp())?;
        let expires_at = timestamp(certificate.validity().not_after.timestamp())?;
        if not_before > now
            || expires_at <= now
            || !certificate
                .basic_constraints()
                .map_err(|_| MaterialError::Certificate)?
                .is_some_and(|bc| bc.value.ca)
            || certificate
                .key_usage()
                .map_err(|_| MaterialError::Certificate)?
                .is_some_and(|usage| !usage.value.key_cert_sign())
        {
            continue;
        }
        usable.push(root.clone());
        expiries.push(expires_at);
    }
    let anchors = usable
        .iter()
        .map(webpki::anchor_from_trusted_cert)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| MaterialError::Certificate)?;
    let end_entity = webpki::EndEntityCert::try_from(leaf).map_err(|_| MaterialError::Certificate)?;
    let time = UnixTime::since_unix_epoch(
        now.duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|_| MaterialError::Profile)?,
    );
    let provider = rustls::crypto::aws_lc_rs::default_provider();
    let usage = if server {
        webpki::KeyUsage::server_auth()
    } else {
        webpki::KeyUsage::client_auth()
    };
    let path = end_entity
        .verify_for_usage(
            provider.signature_verification_algorithms.all,
            &anchors,
            &chain[1..],
            time,
            usage,
            None,
            None,
        )
        .map_err(|_| MaterialError::Certificate)?;
    let mut expiry = leaf_expiry;
    for certificate in path.intermediate_certificates() {
        let der = certificate.der();
        let (_, parsed) = X509Certificate::from_der(der.as_ref()).map_err(|_| MaterialError::Certificate)?;
        expiry = expiry.min(timestamp(parsed.validity().not_after.timestamp())?);
    }
    let root_expiry = anchors
        .iter()
        .zip(expiries)
        .find(|(anchor, _)| std::ptr::eq(*anchor, path.anchor()))
        .map(|(_, expiry)| expiry)
        .ok_or(MaterialError::Certificate)?;
    Ok(expiry.min(root_expiry))
}

pub(crate) fn server_config(
    candidate: &Candidate,
    registration: &RegistrationSet,
    authority: MaterialAuthority,
) -> Result<Arc<ServerConfig>, MaterialError> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = RootCertStore::empty();
    for root in &candidate.roots {
        roots.add(root.clone()).map_err(|_| MaterialError::Certificate)?;
    }
    let inner = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
        .build()
        .map_err(|_| MaterialError::Certificate)?;
    let verifier = RegisteredClientVerifier {
        inner,
        roots: candidate.roots.clone(),
        registration: registration.clone(),
        authority: authority.clone(),
    };
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| MaterialError::Certificate)?
        .with_client_cert_verifier(Arc::new(verifier))
        .with_cert_resolver(Arc::new(LiveResolver::new(&candidate.certified_key, authority)));
    config.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    config.send_tls13_tickets = 0;
    Ok(Arc::new(config))
}

pub(crate) fn client_config(
    candidate: &Candidate,
    registration: &RegistrationSet,
    expected: &SpiffeId,
    authority: MaterialAuthority,
) -> Result<Arc<ClientConfig>, MaterialError> {
    if !registration
        .bindings()
        .iter()
        .any(|binding| binding.spiffe_id() == expected)
    {
        return Err(MaterialError::Unavailable);
    }
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = RegisteredServerVerifier {
        roots: candidate.roots.clone(),
        expected: expected.clone(),
        algorithms: provider.signature_verification_algorithms,
        authority: authority.clone(),
    };
    let mut config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| MaterialError::Certificate)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_client_cert_resolver(Arc::new(LiveResolver::new(&candidate.certified_key, authority)));
    config.resumption = rustls::client::Resumption::disabled();
    Ok(Arc::new(config))
}

#[derive(Debug)]
struct RegisteredClientVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    roots: Vec<CertificateDer<'static>>,
    registration: RegistrationSet,
    authority: MaterialAuthority,
}

impl ClientCertVerifier for RegisteredClientVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }
    fn verify_client_cert(
        &self,
        leaf: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.authority.require_current()?;
        let now = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(now.as_secs()))
            .ok_or_else(|| refused(MaterialError::Profile))?;
        let (id, _, _) = leaf_profile(leaf, now).map_err(refused)?;
        if !self
            .registration
            .bindings()
            .iter()
            .any(|binding| binding.spiffe_id() == &id && binding.application_id().is_some())
        {
            return Err(refused(MaterialError::Identity));
        }
        let chain: Vec<_> = std::iter::once(leaf.clone())
            .chain(intermediates.iter().cloned())
            .collect();
        verify_chain(&chain, &self.roots, now, false).map_err(refused)?;
        let verified = self.inner.verify_client_cert(
            leaf,
            intermediates,
            UnixTime::since_unix_epoch(
                now.duration_since(SystemTime::UNIX_EPOCH)
                    .map_err(|_| refused(MaterialError::Profile))?,
            ),
        )?;
        self.authority.require_current()?;
        Ok(verified)
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.authority.require_current()?;
        let verified = self.inner.verify_tls12_signature(message, cert, dss)?;
        self.authority.require_current()?;
        Ok(verified)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.authority.require_current()?;
        let verified = self.inner.verify_tls13_signature(message, cert, dss)?;
        self.authority.require_current()?;
        Ok(verified)
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

#[derive(Debug)]
struct RegisteredServerVerifier {
    roots: Vec<CertificateDer<'static>>,
    expected: SpiffeId,
    algorithms: rustls::crypto::WebPkiSupportedAlgorithms,
    authority: MaterialAuthority,
}

impl ServerCertVerifier for RegisteredServerVerifier {
    fn verify_server_cert(
        &self,
        leaf: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.authority.require_current()?;
        let now = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(now.as_secs()))
            .ok_or_else(|| refused(MaterialError::Profile))?;
        let (id, _, _) = leaf_profile(leaf, now).map_err(refused)?;
        if id != self.expected {
            return Err(refused(MaterialError::Identity));
        }
        let chain: Vec<_> = std::iter::once(leaf.clone())
            .chain(intermediates.iter().cloned())
            .collect();
        verify_chain(&chain, &self.roots, now, true).map_err(refused)?;
        self.authority.require_current()?;
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.authority.require_current()?;
        let verified = rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algorithms)?;
        self.authority.require_current()?;
        Ok(verified)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.authority.require_current()?;
        let verified = rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algorithms)?;
        self.authority.require_current()?;
        Ok(verified)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algorithms.supported_schemes()
    }
}

fn refused(_: MaterialError) -> rustls::Error {
    rustls::Error::InvalidCertificate(rustls::CertificateError::ApplicationVerificationFailure)
}
