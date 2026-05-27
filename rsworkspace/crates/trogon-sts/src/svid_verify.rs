use std::time::{SystemTime, UNIX_EPOCH};

use x509_parser::extensions::GeneralName;
use x509_parser::pem::Pem;
use x509_parser::prelude::{FromDer, X509Certificate};
use x509_parser::time::ASN1Time;

use crate::error::StsError;
use crate::spiffe_id::{SpiffeId, SpiffeIdError};
use crate::workload_svid::WorkloadSvid;

pub fn leaf_der_from_actor_token(actor_token: &str) -> Result<Vec<u8>, StsError> {
    let trimmed = actor_token.trim();
    if trimmed.starts_with("spiffe://") || trimmed.starts_with("sha256:") || trimmed.starts_with("eyJ") {
        return Err(StsError::InvalidGrant(
            "X.509 SVID required: pass PEM certificate in actor_token".into(),
        ));
    }
    for pem_result in Pem::iter_from_buffer(trimmed.as_bytes()) {
        let pem = pem_result.map_err(|e| StsError::InvalidGrant(format!("actor_token PEM: {e}")))?;
        if pem.label.to_uppercase().contains("CERTIFICATE") {
            return Ok(pem.contents);
        }
    }
    Err(StsError::InvalidGrant(
        "actor_token contained no CERTIFICATE PEM block".into(),
    ))
}

pub fn spiffe_id_from_leaf_der(leaf_der: &[u8]) -> Result<SpiffeId, StsError> {
    let (_, leaf) = X509Certificate::from_der(leaf_der)
        .map_err(|e| StsError::InvalidGrant(format!("invalid SVID certificate: {e}")))?;
    extract_spiffe_id(&leaf)
}

pub fn verify_leaf_against_bundle(leaf_der: &[u8], trust_bundle_pem: &str) -> Result<(), StsError> {
    let (_, leaf) = X509Certificate::from_der(leaf_der)
        .map_err(|e| StsError::InvalidGrant(format!("invalid SVID certificate: {e}")))?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let asn1_now = ASN1Time::from_timestamp(now)
        .map_err(|e| StsError::ServerError(format!("clock: {e}")))?;
    if !leaf.validity().is_valid_at(asn1_now) {
        return Err(StsError::InvalidGrant(
            "SVID certificate is outside its validity window".into(),
        ));
    }

    let ca_ders = parse_trust_anchor_ders(trust_bundle_pem)?;
    let mut trusted = false;
    for ca_der in &ca_ders {
        let (_, ca) = X509Certificate::from_der(ca_der)
            .map_err(|e| StsError::ServerError(format!("trust anchor: {e}")))?;
        if leaf.issuer() != ca.subject() {
            continue;
        }
        leaf.verify_signature(Some(&ca.tbs_certificate.subject_pki))
            .map_err(|e| StsError::InvalidGrant(format!("SVID signature: {e}")))?;
        trusted = true;
        break;
    }
    if !trusted {
        return Err(StsError::InvalidGrant(
            "SVID certificate is not issued by a configured trust anchor".into(),
        ));
    }
    Ok(())
}

pub fn workload_svid_from_pem(actor_token: &str, trust_bundle_pem: &str) -> Result<WorkloadSvid, StsError> {
    let leaf_der = leaf_der_from_actor_token(actor_token)?;
    verify_leaf_against_bundle(&leaf_der, trust_bundle_pem)?;
    let spiffe_id = spiffe_id_from_leaf_der(&leaf_der)?;
    Ok(WorkloadSvid::new(
        spiffe_id,
        leaf_der,
        actor_token.trim().to_string(),
    ))
}

fn parse_trust_anchor_ders(bundle_pem: &str) -> Result<Vec<Vec<u8>>, StsError> {
    let pems: Vec<Pem> = Pem::iter_from_buffer(bundle_pem.as_bytes())
        .map(|r| r.map_err(|e| StsError::ServerError(format!("trust bundle PEM: {e}"))))
        .collect::<Result<Vec<_>, _>>()?;
    let mut cas = Vec::new();
    for pem in pems {
        if !pem.label.to_uppercase().contains("CERTIFICATE") {
            continue;
        }
        cas.push(pem.contents);
    }
    if cas.is_empty() {
        return Err(StsError::ServerError(
            "trust bundle contained no certificates".into(),
        ));
    }
    Ok(cas)
}

fn extract_spiffe_id(leaf: &X509Certificate<'_>) -> Result<SpiffeId, StsError> {
    let san = leaf
        .subject_alternative_name()
        .map_err(|e| StsError::InvalidGrant(format!("SVID SAN parse: {e}")))?
        .ok_or_else(|| {
            StsError::InvalidGrant(
                "SVID certificate has no Subject Alternative Name extension".into(),
            )
        })?;
    for name in san.value.general_names.iter() {
        if let GeneralName::URI(uri) = name
            && uri.starts_with("spiffe://")
        {
            return SpiffeId::parse(uri).map_err(map_spiffe_err);
        }
    }

    Err(StsError::InvalidGrant(
        "SVID certificate has no SPIFFE URI in Subject Alternative Name".into(),
    ))
}

fn map_spiffe_err(e: SpiffeIdError) -> StsError {
    StsError::InvalidGrant(format!("invalid SPIFFE ID in SVID: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        SanType,
    };

    fn spiffe_svid_pem(trust_domain: &str, path: &str) -> (String, String) {
        use rcgen::Ia5String;
        let spiffe_uri: Ia5String = format!("spiffe://{trust_domain}/{path}")
            .try_into()
            .expect("spiffe uri");
        let ca_key = KeyPair::generate().expect("ca key");
        let mut ca_params = CertificateParams::default();
        ca_params.distinguished_name.push(DnType::CommonName, "test-ca");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca = ca_params.self_signed(&ca_key).expect("ca");

        let ee_key = KeyPair::generate().expect("ee key");
        let mut ee_params = CertificateParams::default();
        ee_params.distinguished_name.push(DnType::CommonName, "workload");
        ee_params.subject_alt_names = vec![SanType::URI(spiffe_uri)];
        ee_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let ee = ee_params.signed_by(&ee_key, &ca, &ca_key).expect("ee");
        (ee.pem(), ca.pem())
    }

    #[test]
    fn verifies_rcgen_svid_and_extracts_spiffe_id() {
        let (leaf, anchors) = spiffe_svid_pem("acme.local", "ns/prod/sa/oncall-agent");
        let svid = workload_svid_from_pem(&leaf, &anchors).expect("verify");
        assert_eq!(
            svid.spiffe_id.as_str(),
            "spiffe://acme.local/ns/prod/sa/oncall-agent"
        );
        assert_eq!(svid.wkl(), "spiffe://acme.local/ns/prod/sa/oncall-agent");
    }

    #[test]
    fn rejects_wrong_anchor() {
        let (leaf, _) = spiffe_svid_pem("acme.local", "ns/prod/sa/x");
        let (_, other_ca) = spiffe_svid_pem("other.local", "ns/x");
        let err = workload_svid_from_pem(&leaf, &other_ca).unwrap_err();
        assert!(matches!(err, StsError::InvalidGrant(_)));
    }
}
