use crate::{CredentialExpiry, VerifiedCredential};
use std::time::Duration;

use rustls_pki_types::{CertificateDer, UnixTime};
use serde::Serialize;
use time::OffsetDateTime;
use x509_parser::pem::Pem;
use x509_parser::prelude::FromDer;
use x509_parser::prelude::X509Certificate;
use x509_parser::time::ASN1Time;

use crate::error::{AuthCalloutError, CredentialError};
use crate::jwt::{
    AudienceAccount, CallerNamespace, ExternalSubject, UserJwtClaims, derive_caller_id, external_subject_from_der,
    spicedb_bundle_for_opaque,
};

mod certificate_verification_error;
pub use certificate_verification_error::CertificateVerificationError;

/// Principal document minted for an mTLS-authenticated caller: the SpiceDB
/// subject derived from the certificate, tagged with the scheme that proved it.
#[derive(Debug, Serialize)]
struct MtlsPrincipal<'a> {
    spicedb_subject: &'a str,
    mtls: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCertPem(String);

impl ClientCertPem {
    pub fn new(pem: impl Into<String>) -> Self {
        Self(pem.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustAnchorPem(String);

impl TrustAnchorPem {
    pub fn new(bundle: impl Into<String>) -> Self {
        Self(bundle.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

pub struct X509MtlsVerifier {
    anchors: TrustAnchorPem,
}

impl X509MtlsVerifier {
    pub fn new(anchors: TrustAnchorPem) -> Self {
        Self { anchors }
    }

    /// Collect every CERTIFICATE block in the client PEM in order. The first
    /// is the leaf; any remaining are intermediates between leaf and a
    /// configured trust anchor.
    fn chain_ders(pem: &str) -> Result<Vec<Vec<u8>>, AuthCalloutError> {
        let mut out = Vec::new();
        for pem_result in Pem::iter_from_buffer(pem.as_bytes()) {
            let pem = pem_result.map_err(|e| CredentialError::InvalidCredentials(format!("PEM parse error: {e}")))?;
            if pem.label.to_uppercase().contains("CERTIFICATE") {
                out.push(pem.contents);
            }
        }
        if out.is_empty() {
            return Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidCredentials("client certificate PEM contained no CERTIFICATE block".into()),
            ));
        }
        Ok(out)
    }

    fn anchor_pems(bundle: &str) -> Result<Vec<Pem>, AuthCalloutError> {
        Pem::iter_from_buffer(bundle.as_bytes())
            .map(|r| {
                r.map_err(|e| -> AuthCalloutError {
                    CredentialError::InvalidCredentials(format!("trust anchor PEM: {e}")).into()
                })
            })
            .collect()
    }

    fn parse_cas(pems: &[Pem]) -> Result<Vec<X509Certificate<'_>>, AuthCalloutError> {
        let mut out = Vec::new();
        for pem in pems {
            if !pem.label.to_uppercase().contains("CERTIFICATE") {
                continue;
            }
            let x509 = pem
                .parse_x509()
                .map_err(|e| CredentialError::InvalidCredentials(format!("invalid trust anchor cert DER: {e}")))?;
            out.push(x509);
        }
        if out.is_empty() {
            return Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidCredentials("trust anchor bundle contained no certificates".into()),
            ));
        }
        Ok(out)
    }

    pub fn verify_sync(
        &self,
        cert: &ClientCertPem,
        account: &AudienceAccount,
        now: OffsetDateTime,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        let chain_ders = Self::chain_ders(cert.as_str())?;
        let leaf_der = chain_ders[0].clone();
        let chain: Vec<X509Certificate<'_>> = chain_ders
            .iter()
            .map(|d| {
                X509Certificate::from_der(d)
                    .map(|(_, c)| c)
                    .map_err(|e| CredentialError::InvalidCredentials(format!("invalid client chain cert: {e}")))
            })
            .collect::<Result<_, _>>()?;
        let leaf = &chain[0];
        let anchor_pems = Self::anchor_pems(self.anchors.as_str())?;
        let cas = Self::parse_cas(&anchor_pems)?;

        if !leaf.validity().is_valid_at(ASN1Time::from(now)) {
            return Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidCredentials(
                    "client certificate validity window does not include verification time".into(),
                ),
            ));
        }

        // Refuse to mint for a CA or intermediate that someone presented as
        // an "end entity" — only certs explicitly marked non-CA can be the
        // client identity. RFC 5280 §4.2.1.9: basicConstraints.cA=false (or
        // absent) means end-entity. We treat "absent" as end-entity since
        // it's the common shape for client certs.
        if let Ok(Some(bc)) = leaf.basic_constraints()
            && bc.value.ca
        {
            return Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidCredentials("client certificate is a CA, expected end-entity".into()),
            ));
        }

        let expires_at = Self::verify_client_chain(&chain_ders, &chain, &anchor_pems, &cas, now)
            .map_err(CredentialError::MtlsCertificate)?;
        let expires_at =
            CredentialExpiry::from_unix_seconds(expires_at.unix_timestamp()).map_err(CredentialError::Validity)?;

        let sub = ExternalSubject::from_x509(leaf, &leaf_der)
            .map_err(|e| CredentialError::InvalidCredentials(format!("mTLS subject extraction failed: {e}")))?;
        let principal = MtlsPrincipal {
            spicedb_subject: sub.as_str(),
            mtls: true,
        };
        let data = spicedb_bundle_for_opaque(
            serde_json::to_value(principal).map_err(CredentialError::PrincipalSerialization)?,
        );
        let namespace = CallerNamespace::Mtls;
        let caller_id = derive_caller_id(namespace, sub.as_str(), account)
            .map_err(|e| CredentialError::InvalidCredentials(format!("caller_id derivation failed: {e}")))?;
        Ok(VerifiedCredential::new(
            UserJwtClaims {
                kid: crate::signing_key_source::unminted_placeholder(),
                sub,
                aud: account.clone(),
                data,
                caller_id,
                principal_kind: namespace.principal_kind(),
                nats_permissions: crate::permissions::IssuedPermissions::none(),
            },
            expires_at,
        ))
    }

    fn verify_client_chain(
        chain_ders: &[Vec<u8>],
        chain: &[X509Certificate<'_>],
        anchor_pems: &[Pem],
        cas: &[X509Certificate<'_>],
        now: OffsetDateTime,
    ) -> Result<OffsetDateTime, CertificateVerificationError> {
        let certificates: Vec<_> = chain_ders
            .iter()
            .map(|der| CertificateDer::from(der.as_slice()))
            .collect();
        let end_entity = webpki::EndEntityCert::try_from(&certificates[0])?;
        let leaf = &chain[0];
        if leaf
            .basic_constraints()?
            .is_some_and(|constraints| constraints.value.ca)
        {
            return Err(webpki::Error::CaUsedAsEndEntity.into());
        }
        if leaf.key_usage()?.is_some_and(|usage| !usage.value.digital_signature()) {
            return Err(CertificateVerificationError::LeafKeyUsage);
        }
        if leaf.extended_key_usage()?.is_some_and(|usage| !usage.value.client_auth) {
            return Err(CertificateVerificationError::ClientAuthUsage);
        }

        // A configured end-entity certificate pins exactly those bytes. It
        // cannot become a CA for another certificate, even with the same key.
        if anchor_pems
            .iter()
            .filter(|pem| pem.label.to_uppercase().contains("CERTIFICATE"))
            .any(|pem| pem.contents == chain_ders[0])
        {
            return Ok(leaf.validity().not_after.to_datetime());
        }

        for issuer in &chain[1..] {
            if issuer.key_usage()?.is_some_and(|usage| !usage.value.key_cert_sign()) {
                return Err(CertificateVerificationError::IssuerKeyUsage);
            }
        }

        let mut anchor_ders = Vec::new();
        let mut anchor_expiries = Vec::new();
        let certificate_pems = anchor_pems
            .iter()
            .filter(|pem| pem.label.to_uppercase().contains("CERTIFICATE"));
        for (ca, pem) in cas.iter().zip(certificate_pems) {
            if !ca.validity().is_valid_at(ASN1Time::from(now))
                || !ca.basic_constraints()?.is_some_and(|constraints| constraints.value.ca)
            {
                continue;
            }
            if ca.key_usage()?.is_some_and(|usage| !usage.value.key_cert_sign()) {
                continue;
            }
            anchor_ders.push(CertificateDer::from(pem.contents.as_slice()));
            anchor_expiries.push(ca.validity().not_after.to_datetime());
        }
        let anchors = anchor_ders
            .iter()
            .map(webpki::anchor_from_trusted_cert)
            .collect::<Result<Vec<_>, _>>()?;
        let timestamp = u64::try_from(now.unix_timestamp())?;
        let path = end_entity.verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &anchors,
            &certificates[1..],
            UnixTime::since_unix_epoch(Duration::from_secs(timestamp)),
            webpki::KeyUsage::client_auth(),
            None,
            None,
        )?;
        let mut expires_at = leaf.validity().not_after.to_datetime();
        for certificate in path.intermediate_certificates() {
            let der = certificate.der();
            let parsed = chain
                .iter()
                .zip(chain_ders)
                .find(|(_, candidate)| candidate.as_slice() == der.as_ref())
                .map(|(parsed, _)| parsed)
                .ok_or(CertificateVerificationError::PathMetadata)?;
            expires_at = expires_at.min(parsed.validity().not_after.to_datetime());
        }
        let anchor_expiry = anchors
            .iter()
            .zip(anchor_expiries)
            .find(|(anchor, _)| std::ptr::eq(*anchor, path.anchor()))
            .map(|(_, expiry)| expiry)
            .ok_or(CertificateVerificationError::PathMetadata)?;
        Ok(expires_at.min(anchor_expiry))
    }
}

trait ExternalSubjectExt {
    fn from_x509(leaf: &X509Certificate<'_>, leaf_der: &[u8]) -> Result<ExternalSubject, AuthCalloutError>;
}

impl ExternalSubjectExt for ExternalSubject {
    fn from_x509(leaf: &X509Certificate<'_>, leaf_der: &[u8]) -> Result<ExternalSubject, AuthCalloutError> {
        let dn = leaf.subject().to_string();
        if dn.is_empty() {
            return external_subject_from_der("mtls", leaf_der).map_err(|e| {
                AuthCalloutError::from(CredentialError::InvalidCredentials(format!(
                    "fallback subject encoding failed: {e}"
                )))
            });
        }
        ExternalSubject::new(dn).map_err(|e| {
            AuthCalloutError::from(CredentialError::InvalidCredentials(format!(
                "invalid external subject from DN: {e}"
            )))
        })
    }
}

#[async_trait::async_trait]
pub trait MTlsVerifier: Send + Sync + 'static {
    async fn verify(
        &self,
        cert: &ClientCertPem,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError>;
}

#[async_trait::async_trait]
impl MTlsVerifier for X509MtlsVerifier {
    async fn verify(
        &self,
        cert: &ClientCertPem,
        account: &AudienceAccount,
    ) -> Result<VerifiedCredential, AuthCalloutError> {
        let now = OffsetDateTime::now_utc();
        Ok(self.verify_sync(cert, account, now)?)
    }
}

#[cfg(test)]
mod tests;
