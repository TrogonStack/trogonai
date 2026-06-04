//! mTLS client-certificate verifier with SPIFFE SAN extraction.
//!
//! Composes the default rustls `WebPkiClientVerifier` with an extra
//! SPIFFE-trust-domain check. When `spiffe_enabled = true`, the cert
//! MUST carry a URI SAN that parses as a SPIFFE ID and whose trust
//! domain matches the configured allowlist. When `spiffe_enabled =
//! false`, falls back to DNS SAN pattern matching against
//! `allowed_dns_patterns`.

use std::sync::Arc;

use rustls::DigitallySignedStruct;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{CertificateDer, UnixTime};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DistinguishedName, RootCertStore, SignatureScheme};
use trogon_sts::spiffe_id::{SpiffeId, SpiffeIdError};
use x509_parser::extensions::{GeneralName, ParsedExtension};
use x509_parser::prelude::*;

/// Stable error codes surfaced to the auth-callout / structured log
/// path. Mirrors the constants the scaffold tests pin.
pub const ERR_TLS_REQUIRED: &str = "tls_required";
pub const ERR_MTLS_REQUIRED: &str = "mtls_required";
pub const ERR_MTLS_SAN_MISMATCH: &str = "mtls_san_mismatch";
pub const ERR_TLS_VERSION_UNSUPPORTED: &str = "tls_version_unsupported";

/// Minimum TLS protocol version accepted by the gateway. We pin 1.3
/// because rustls otherwise advertises 1.2 by default.
pub const TLS_MIN_VERSION: &str = "1.3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiffeIdentity {
    pub spiffe_id: SpiffeId,
}

#[derive(Debug, thiserror::Error)]
pub enum SpiffeVerifierError {
    #[error("client cert chain is empty")]
    EmptyChain,
    #[error("client cert could not be parsed: {0}")]
    CertParse(String),
    #[error("client cert is missing the URI SAN required when spiffe_enabled=true")]
    UriSanMissing,
    #[error("URI SAN `{0}` is not a valid SPIFFE ID: {1}")]
    BadSpiffeId(String, SpiffeIdError),
    #[error("SPIFFE trust domain `{0}` is not in the allowlist")]
    TrustDomainNotAllowed(String),
    #[error("no DNS SAN matched any configured allow pattern")]
    DnsSanMismatch,
    #[error("rustls inner verifier rejected the cert: {0}")]
    Inner(String),
}

/// Composite verifier: rustls PKI chain check + SPIFFE/DNS SAN
/// policy. Wrap as `Arc<dyn ClientCertVerifier>` when wiring into a
/// `ServerConfig`.
#[derive(Debug)]
pub struct SpiffeClientCertVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    spiffe_enabled: bool,
    trust_domains: Vec<String>,
    allowed_dns_patterns: Vec<DnsPattern>,
}

impl SpiffeClientCertVerifier {
    /// Build the verifier from a trust bundle plus policy.
    ///
    /// `trust_domains` empty + `spiffe_enabled=true` means "any
    /// SPIFFE ID accepted." Most deployments should configure at
    /// least one domain.
    pub fn new(
        roots: RootCertStore,
        spiffe_enabled: bool,
        trust_domains: Vec<String>,
        allowed_dns_patterns: Vec<String>,
    ) -> Result<Arc<Self>, rustls::Error> {
        let inner = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| rustls::Error::General(e.to_string()))?;
        let patterns = allowed_dns_patterns.into_iter().map(DnsPattern::parse).collect();
        Ok(Arc::new(Self {
            inner,
            spiffe_enabled,
            trust_domains,
            allowed_dns_patterns: patterns,
        }))
    }

    /// Pure-logic SAN check separated from the rustls trait so unit
    /// tests can exercise it without wiring up a full handshake.
    pub fn check_san(&self, end_entity_der: &[u8]) -> Result<SpiffeIdentity, SpiffeVerifierError> {
        let (_, parsed) =
            X509Certificate::from_der(end_entity_der).map_err(|e| SpiffeVerifierError::CertParse(e.to_string()))?;

        let san = parsed
            .extensions()
            .iter()
            .find(|ext| matches!(ext.parsed_extension(), ParsedExtension::SubjectAlternativeName(_)));

        let san_names: Vec<&GeneralName<'_>> = match san.map(|s| s.parsed_extension()) {
            Some(ParsedExtension::SubjectAlternativeName(san)) => san.general_names.iter().collect(),
            _ => Vec::new(),
        };

        if self.spiffe_enabled {
            let uri = san_names
                .iter()
                .find_map(|n| match n {
                    GeneralName::URI(u) => Some(*u),
                    _ => None,
                })
                .ok_or(SpiffeVerifierError::UriSanMissing)?;
            let id = SpiffeId::parse(uri).map_err(|e| SpiffeVerifierError::BadSpiffeId(uri.to_string(), e))?;
            if !self.trust_domains.is_empty() && !self.trust_domains.iter().any(|td| td == id.trust_domain()) {
                return Err(SpiffeVerifierError::TrustDomainNotAllowed(id.trust_domain().to_string()));
            }
            Ok(SpiffeIdentity { spiffe_id: id })
        } else {
            let dns: Vec<&str> = san_names
                .iter()
                .filter_map(|n| match n {
                    GeneralName::DNSName(d) => Some(*d),
                    _ => None,
                })
                .collect();
            if dns.is_empty() {
                return Err(SpiffeVerifierError::DnsSanMismatch);
            }
            let matched = dns
                .iter()
                .any(|name| self.allowed_dns_patterns.iter().any(|p| p.matches(name)));
            if !matched {
                return Err(SpiffeVerifierError::DnsSanMismatch);
            }
            // Synthesize a stable identity from the matching DNS name
            // so downstream callers can audit it.
            let synthetic = format!("spiffe://dns/{}", dns[0]);
            Ok(SpiffeIdentity {
                spiffe_id: SpiffeId::parse(&synthetic).map_err(|e| SpiffeVerifierError::BadSpiffeId(synthetic, e))?,
            })
        }
    }
}

impl ClientCertVerifier for SpiffeClientCertVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.inner.verify_client_cert(end_entity, intermediates, now)?;
        self.check_san(end_entity.as_ref())
            .map_err(|e| rustls::Error::General(e.to_string()))?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General(ERR_TLS_VERSION_UNSUPPORTED.into()))
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
}

#[derive(Debug, Clone)]
struct DnsPattern {
    raw: String,
    wildcard: bool,
    suffix: String,
}

impl DnsPattern {
    fn parse(raw: String) -> Self {
        if let Some(rest) = raw.strip_prefix("*.") {
            Self {
                wildcard: true,
                suffix: format!(".{rest}"),
                raw,
            }
        } else {
            Self {
                wildcard: false,
                suffix: raw.clone(),
                raw,
            }
        }
    }

    fn matches(&self, name: &str) -> bool {
        if self.wildcard {
            name.ends_with(&self.suffix) && !name[..name.len() - self.suffix.len()].contains('.')
        } else {
            self.raw == name
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertificateParams, KeyPair, SanType};

    fn cert_with_spiffe(uri: &str) -> Vec<u8> {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params.subject_alt_names.push(SanType::URI(uri.try_into().unwrap()));
        let kp = KeyPair::generate().unwrap();
        let cert = params.self_signed(&kp).unwrap();
        cert.der().to_vec()
    }

    fn cert_with_dns(name: &str) -> Vec<u8> {
        let params = CertificateParams::new(vec![name.to_string()]).unwrap();
        let kp = KeyPair::generate().unwrap();
        let cert = params.self_signed(&kp).unwrap();
        cert.der().to_vec()
    }

    fn verifier(spiffe_enabled: bool, trust_domains: Vec<&str>, dns: Vec<&str>) -> SpiffeClientCertVerifier {
        SpiffeClientCertVerifier {
            inner: Arc::new(StubVerifier),
            spiffe_enabled,
            trust_domains: trust_domains.into_iter().map(String::from).collect(),
            allowed_dns_patterns: dns.into_iter().map(|s| DnsPattern::parse(s.into())).collect(),
        }
    }

    #[derive(Debug)]
    struct StubVerifier;
    impl ClientCertVerifier for StubVerifier {
        fn root_hint_subjects(&self) -> &[DistinguishedName] {
            &[]
        }
        fn verify_client_cert(
            &self,
            _: &CertificateDer<'_>,
            _: &[CertificateDer<'_>],
            _: UnixTime,
        ) -> Result<ClientCertVerified, rustls::Error> {
            Ok(ClientCertVerified::assertion())
        }
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Err(rustls::Error::General("tls 1.2 not allowed".into()))
        }
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &CertificateDer<'_>,
            _: &DigitallySignedStruct,
        ) -> Result<HandshakeSignatureValid, rustls::Error> {
            Ok(HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
            vec![SignatureScheme::ED25519]
        }
    }

    #[test]
    fn spiffe_id_in_allowed_domain_accepted() {
        let v = verifier(true, vec!["acme.local"], vec![]);
        let der = cert_with_spiffe("spiffe://acme.local/ns/prod/sa/agent");
        let id = v.check_san(&der).unwrap();
        assert_eq!(id.spiffe_id.trust_domain(), "acme.local");
    }

    #[test]
    fn spiffe_id_outside_trust_domain_rejected() {
        let v = verifier(true, vec!["acme.local"], vec![]);
        let der = cert_with_spiffe("spiffe://attacker.example/ns/x");
        let err = v.check_san(&der).unwrap_err();
        assert!(matches!(err, SpiffeVerifierError::TrustDomainNotAllowed(_)), "got {err:?}");
    }

    #[test]
    fn spiffe_enabled_rejects_cert_without_uri_san() {
        let v = verifier(true, vec!["acme.local"], vec![]);
        let der = cert_with_dns("agent.acme.local");
        let err = v.check_san(&der).unwrap_err();
        assert!(matches!(err, SpiffeVerifierError::UriSanMissing));
    }

    #[test]
    fn dns_san_exact_match_accepted_when_spiffe_disabled() {
        let v = verifier(false, vec![], vec!["agent.acme.local"]);
        let der = cert_with_dns("agent.acme.local");
        let id = v.check_san(&der).unwrap();
        assert_eq!(id.spiffe_id.path(), "agent.acme.local");
    }

    #[test]
    fn dns_san_wildcard_match_accepted() {
        let v = verifier(false, vec![], vec!["*.agents.example.com"]);
        let der = cert_with_dns("foo.agents.example.com");
        v.check_san(&der).unwrap();
    }

    #[test]
    fn dns_san_wildcard_does_not_cross_label() {
        let v = verifier(false, vec![], vec!["*.agents.example.com"]);
        let der = cert_with_dns("a.b.agents.example.com");
        let err = v.check_san(&der).unwrap_err();
        assert!(matches!(err, SpiffeVerifierError::DnsSanMismatch));
    }

    #[test]
    fn dns_san_mismatch_rejected() {
        let v = verifier(false, vec![], vec!["agent.acme.local"]);
        let der = cert_with_dns("other.example.com");
        let err = v.check_san(&der).unwrap_err();
        assert!(matches!(err, SpiffeVerifierError::DnsSanMismatch));
    }

    #[test]
    fn empty_trust_domain_allowlist_means_any_domain() {
        let v = verifier(true, vec![], vec![]);
        let der = cert_with_spiffe("spiffe://any.example/ns/x");
        v.check_san(&der).unwrap();
    }
}
