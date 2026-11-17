use std::sync::Arc;
use std::time::Duration;
use std::time::SystemTime;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, DistinguishedName, RootCertStore, ServerConfig, SignatureScheme};
use spiffe::SpiffeId;

use crate::candidate::Candidate;
use crate::certificate_validation::{leaf_profile, verify_chain};
use crate::material_authority::MaterialAuthority;
use crate::tls_guards::LiveResolver;
use crate::{MaterialError, RegistrationSet};

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
        verify_chain(&chain, &self.roots, now, crate::certificate_use::CertificateUse::Client).map_err(refused)?;
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
        verify_chain(&chain, &self.roots, now, crate::certificate_use::CertificateUse::Server).map_err(refused)?;
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

#[cfg(test)]
mod tests;
