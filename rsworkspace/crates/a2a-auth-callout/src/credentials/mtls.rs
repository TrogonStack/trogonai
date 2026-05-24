use time::OffsetDateTime;
use x509_parser::pem::Pem;
use x509_parser::prelude::FromDer;
use x509_parser::prelude::X509Certificate;
use x509_parser::time::ASN1Time;

use crate::error::AuthCalloutError;
use crate::jwt::{
    derive_caller_id, external_subject_from_der, spicedb_bundle_for_opaque, AudienceAccount, ExternalSubject,
    UserJwtClaims,
};

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

    fn leaf_der(pem: &str) -> Result<Vec<u8>, AuthCalloutError> {
        for pem_result in Pem::iter_from_buffer(pem.as_bytes()) {
            let pem = pem_result
                .map_err(|e| AuthCalloutError::CredentialVerification(format!("PEM parse error: {e}")))?;
            if pem.label.to_uppercase().contains("CERTIFICATE") {
                return Ok(pem.contents);
            }
        }
        Err(AuthCalloutError::CredentialVerification(
            "client certificate PEM contained no CERTIFICATE block".into(),
        ))
    }

    fn anchor_pems(bundle: &str) -> Result<Vec<Pem>, AuthCalloutError> {
        Pem::iter_from_buffer(bundle.as_bytes())
            .map(|r| {
                r.map_err(|e| AuthCalloutError::CredentialVerification(format!("trust anchor PEM: {e}")))
            })
            .collect()
    }

    fn parse_cas(pems: &[Pem]) -> Result<Vec<X509Certificate<'_>>, AuthCalloutError> {
        let mut out = Vec::new();
        for pem in pems {
            if !pem.label.to_uppercase().contains("CERTIFICATE") {
                continue;
            }
            let x509 = pem.parse_x509().map_err(|e| {
                AuthCalloutError::CredentialVerification(format!("invalid trust anchor cert DER: {e}"))
            })?;
            out.push(x509);
        }
        if out.is_empty() {
            return Err(AuthCalloutError::CredentialVerification(
                "trust anchor bundle contained no certificates".into(),
            ));
        }
        Ok(out)
    }

    pub fn verify_sync(
        &self,
        cert: &ClientCertPem,
        account: &AudienceAccount,
        now: OffsetDateTime,
    ) -> Result<UserJwtClaims, AuthCalloutError> {
        let leaf_der = Self::leaf_der(cert.as_str())?;
        let (_, leaf) = X509Certificate::from_der(&leaf_der).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("invalid leaf certificate: {e}"))
        })?;
        let anchor_pems = Self::anchor_pems(self.anchors.as_str())?;
        let cas = Self::parse_cas(&anchor_pems)?;

        if !leaf
            .validity()
            .is_valid_at(ASN1Time::from(now))
        {
            return Err(AuthCalloutError::CredentialVerification(
                "client certificate validity window does not include verification time".into(),
            ));
        }

        let mut trusted = false;
        for ca in &cas {
            if leaf.issuer() != ca.subject() {
                continue;
            }
            leaf.verify_signature(Some(&ca.tbs_certificate.subject_pki))
                .map_err(|e| AuthCalloutError::CredentialVerification(format!("certificate signature: {e}")))?;
            trusted = true;
            break;
        }

        if !trusted {
            return Err(AuthCalloutError::CredentialVerification(
                "client certificate is not issued by a configured trust anchor".into(),
            ));
        }

        let sub = ExternalSubject::from_x509(&leaf, &leaf_der).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("mTLS subject extraction failed: {e}"))
        })?;
        let data = spicedb_bundle_for_opaque(serde_json::json!({
            "spicedb_subject": sub.as_str(),
            "mtls": true,
        }));
        let caller_id =
            derive_caller_id(sub.as_str(), account).map_err(|e| {
                AuthCalloutError::CredentialVerification(format!("caller_id derivation failed: {e}"))
            })?;

        let nats_permissions = crate::permissions::IssuedPermissions::default_for_caller(&caller_id);
        Ok(UserJwtClaims {
            kid: crate::signing_key_source::unminted_placeholder(),
            sub,
            aud: account.clone(),
            data,
            caller_id,
            nats_permissions,
        })
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
                AuthCalloutError::CredentialVerification(format!("fallback subject encoding failed: {e}"))
            });
        }
        ExternalSubject::new(dn).map_err(|e| {
            AuthCalloutError::CredentialVerification(format!("invalid external subject from DN: {e}"))
        })
    }
}

#[async_trait::async_trait]
pub trait MTlsVerifier: Send + Sync + 'static {
    async fn verify(
        &self,
        cert: &ClientCertPem,
        account: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError>;
}

#[async_trait::async_trait]
impl MTlsVerifier for X509MtlsVerifier {
    async fn verify(
        &self,
        cert: &ClientCertPem,
        account: &AudienceAccount,
    ) -> Result<UserJwtClaims, AuthCalloutError> {
        let now = OffsetDateTime::now_utc();
        Ok(self.verify_sync(cert, account, now)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_trust_bundle() {
        let empty: Vec<Pem> = Vec::new();
        let err = X509MtlsVerifier::parse_cas(&empty).unwrap_err();
        assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
        let _ = X509MtlsVerifier::new(TrustAnchorPem::new(""));
    }

    #[tokio::test]
    async fn verifies_rcgen_chain() {
        use rcgen::{
            BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
        };

        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_dn = DistinguishedName::new();
        ca_dn.push(DnType::CommonName, "test-ca");
        let mut ca_params = CertificateParams::default();
        ca_params.distinguished_name = ca_dn;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);

        let ca = ca_params.self_signed(&ca_key).expect("ca");

        let ee_key = KeyPair::generate().expect("ee key");
        let mut ee_dn = DistinguishedName::new();
        ee_dn.push(DnType::CommonName, "test-service");
        let mut ee_params = CertificateParams::default();
        ee_params.distinguished_name = ee_dn;

        let ee = ee_params.signed_by(&ee_key, &ca, &ca_key).expect("ee");
        let leaf_pem = ee.pem();
        let anchors = ca.pem();

        let v = X509MtlsVerifier::new(TrustAnchorPem::new(anchors));
        let account = AudienceAccount::new("acct-1");
        let claims = v
            .verify(&ClientCertPem::new(leaf_pem), &account)
            .await
            .expect("verify");
        assert_eq!(claims.aud.as_str(), "acct-1");
        assert!(claims.sub.as_str().contains("test-service"));
        assert!(!claims.caller_id.as_str().contains('.'));
    }

    #[tokio::test]
    async fn rejects_wrong_anchor() {
        use rcgen::{
            BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
        };

        let unrelated_key = KeyPair::generate().expect("k");
        let mut unrelated_dn = DistinguishedName::new();
        unrelated_dn.push(DnType::CommonName, "other-ca");
        let mut unrelated_params = CertificateParams::default();
        unrelated_params.distinguished_name = unrelated_dn;
        unrelated_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let unrelated_ca = unrelated_params.self_signed(&unrelated_key).expect("uca");

        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_dn = DistinguishedName::new();
        ca_dn.push(DnType::CommonName, "real-ca");
        let mut ca_params = CertificateParams::default();
        ca_params.distinguished_name = ca_dn;
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).expect("ca");

        let ee_key = KeyPair::generate().expect("ee");
        let ee = CertificateParams::default()
            .signed_by(&ee_key, &ca, &ca_key)
            .expect("ee");

        let v = X509MtlsVerifier::new(TrustAnchorPem::new(unrelated_ca.pem()));
        let err = v
            .verify(&ClientCertPem::new(ee.pem()), &AudienceAccount::new("a"))
            .await
            .unwrap_err();
        assert!(matches!(err, AuthCalloutError::CredentialVerification(_)));
    }
}
