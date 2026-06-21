use time::OffsetDateTime;
use x509_parser::pem::Pem;
use x509_parser::prelude::FromDer;
use x509_parser::prelude::X509Certificate;
use x509_parser::time::ASN1Time;

use crate::error::{AuthCalloutError, CredentialError};
use crate::jwt::{
    AudienceAccount, ExternalSubject, UserJwtClaims, derive_caller_id, external_subject_from_der,
    spicedb_bundle_for_opaque,
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
    ) -> Result<UserJwtClaims, AuthCalloutError> {
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
        let intermediates = &chain[1..];
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

        let asn1_now = ASN1Time::from(now);
        // Walk leaf → intermediates → trust anchor. At each step, find a
        // currently-valid issuer (preferring trust anchors so a chain that
        // could short-circuit to a configured anchor does so) and verify
        // the signature. The walk caps at chain.len()+1 hops to bound the
        // loop independently of input length.
        let mut current = leaf;
        let mut trusted = false;
        for _ in 0..=chain.len() {
            // If the current cert is itself a configured trust anchor
            // (subject + key match) we're done.
            if cas.iter().any(|ca| {
                ca.subject() == current.subject()
                    && ca.tbs_certificate.subject_pki == current.tbs_certificate.subject_pki
            }) {
                trusted = true;
                break;
            }
            // Look in trust anchors first, then in supplied intermediates.
            let issuer = cas.iter().chain(intermediates.iter()).find(|c| {
                c.subject() == current.issuer()
                    && c.validity().is_valid_at(asn1_now)
                    && current.verify_signature(Some(&c.tbs_certificate.subject_pki)).is_ok()
            });
            let Some(issuer) = issuer else { break };
            if cas.iter().any(|ca| {
                ca.subject() == issuer.subject() && ca.tbs_certificate.subject_pki == issuer.tbs_certificate.subject_pki
            }) {
                trusted = true;
                break;
            }
            current = issuer;
        }

        if !trusted {
            return Err(AuthCalloutError::CredentialVerification(
                CredentialError::InvalidCredentials(
                    "client certificate does not chain to a configured trust anchor".into(),
                ),
            ));
        }

        let sub = ExternalSubject::from_x509(leaf, &leaf_der)
            .map_err(|e| CredentialError::InvalidCredentials(format!("mTLS subject extraction failed: {e}")))?;
        let data = spicedb_bundle_for_opaque(serde_json::json!({
            "spicedb_subject": sub.as_str(),
            "mtls": true,
        }));
        let caller_id = derive_caller_id(sub.as_str(), account)
            .map_err(|e| CredentialError::InvalidCredentials(format!("caller_id derivation failed: {e}")))?;

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
    async fn verify(&self, cert: &ClientCertPem, account: &AudienceAccount) -> Result<UserJwtClaims, AuthCalloutError>;
}

#[async_trait::async_trait]
impl MTlsVerifier for X509MtlsVerifier {
    async fn verify(&self, cert: &ClientCertPem, account: &AudienceAccount) -> Result<UserJwtClaims, AuthCalloutError> {
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
        assert!(matches!(
            err,
            AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
        ));
        let _ = X509MtlsVerifier::new(TrustAnchorPem::new(""));
    }

    #[tokio::test]
    async fn verifies_rcgen_chain() {
        use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

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
        let claims = v.verify(&ClientCertPem::new(leaf_pem), &account).await.expect("verify");
        assert_eq!(claims.aud.as_str(), "acct-1");
        assert!(claims.sub.as_str().contains("test-service"));
        assert!(!claims.caller_id.as_str().contains('.'));
    }

    #[tokio::test]
    async fn rejects_wrong_anchor() {
        use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair};

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
        assert!(matches!(
            err,
            AuthCalloutError::CredentialVerification(CredentialError::InvalidCredentials(_))
        ));
    }
}
