use std::time::{Duration, SystemTime};

use rustls::pki_types::{CertificateDer, UnixTime};
use spiffe::SpiffeId;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::MaterialError;
use crate::certificate_use::CertificateUse;
use crate::constants::MAX_SVID_LIFETIME;

pub(crate) fn timestamp(value: i64) -> Result<SystemTime, MaterialError> {
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(
            u64::try_from(value).map_err(|_| MaterialError::Profile)?,
        ))
        .ok_or(MaterialError::Profile)
}

/// The X.509-SVID key constraints a leaf must satisfy: `cA` false, `digitalSignature`
/// set, neither `keyCertSign` nor `cRLSign`, and, when an extended key usage is
/// present, both `serverAuth` and `clientAuth`. It is the only definition of that
/// profile: [`leaf_profile`] and [`verify_leaf_key_profile`] both call it.
fn key_profile(leaf: &X509Certificate<'_>) -> Result<(), MaterialError> {
    let basic = leaf
        .basic_constraints()
        .map_err(|_| MaterialError::Profile)?
        .ok_or(MaterialError::Profile)?;
    let usage = leaf
        .key_usage()
        .map_err(|_| MaterialError::Profile)?
        .ok_or(MaterialError::Profile)?;
    if basic.value.ca || !usage.value.digital_signature() || usage.value.key_cert_sign() || usage.value.crl_sign() {
        return Err(MaterialError::Profile);
    }
    if leaf
        .extended_key_usage()
        .map_err(|_| MaterialError::Profile)?
        .is_some_and(|eku| !eku.value.client_auth || !eku.value.server_auth)
    {
        return Err(MaterialError::Profile);
    }
    Ok(())
}

/// Checks a DER leaf against the X.509-SVID key constraints only. Validity
/// window, lifetime cap and identity are the caller's concern.
pub fn verify_leaf_key_profile(certificate: &[u8]) -> Result<(), MaterialError> {
    let (rest, leaf) = X509Certificate::from_der(certificate).map_err(|_| MaterialError::Profile)?;
    if !rest.is_empty() {
        return Err(MaterialError::Profile);
    }
    key_profile(&leaf)
}

/// The not-before and not-after instants of a leaf and the positive span
/// between them. A zero or negative span is refused.
fn validity_period(leaf: &X509Certificate<'_>) -> Result<(SystemTime, SystemTime, Duration), MaterialError> {
    let not_before = timestamp(leaf.validity().not_before.timestamp())?;
    let expires_at = timestamp(leaf.validity().not_after.timestamp())?;
    let lifetime = expires_at
        .duration_since(not_before)
        .map_err(|_| MaterialError::Profile)?;
    if lifetime.is_zero() {
        return Err(MaterialError::Profile);
    }
    Ok((not_before, expires_at, lifetime))
}

/// The validity span (not-after minus not-before) of a DER leaf. It is the
/// quantity [`MAX_SVID_LIFETIME`](crate::constants::MAX_SVID_LIFETIME) bounds.
/// Comparing it with that ceiling and checking the current time are the
/// caller's concern.
pub fn leaf_lifetime(certificate: &[u8]) -> Result<Duration, MaterialError> {
    let (rest, leaf) = X509Certificate::from_der(certificate).map_err(|_| MaterialError::Profile)?;
    if !rest.is_empty() {
        return Err(MaterialError::Profile);
    }
    validity_period(&leaf).map(|(_, _, lifetime)| lifetime)
}

pub(crate) fn leaf_profile(
    certificate: &CertificateDer<'_>,
    now: SystemTime,
) -> Result<(SpiffeId, SystemTime, SystemTime), MaterialError> {
    let (rest, leaf) = X509Certificate::from_der(certificate).map_err(|_| MaterialError::Profile)?;
    if !rest.is_empty() {
        return Err(MaterialError::Profile);
    }
    let id = spiffe::cert::spiffe_id_from_der(certificate).map_err(|_| MaterialError::Identity)?;
    if id.path().is_empty() {
        return Err(MaterialError::Identity);
    }
    key_profile(&leaf)?;
    let (not_before, expires_at, lifetime) = validity_period(&leaf)?;
    if not_before > now || expires_at <= now || lifetime > MAX_SVID_LIFETIME {
        return Err(MaterialError::Profile);
    }
    Ok((id, not_before, expires_at))
}

pub(crate) fn verify_chain(
    chain: &[CertificateDer<'_>],
    roots: &[CertificateDer<'_>],
    now: SystemTime,
    purpose: CertificateUse,
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
    let usage = match purpose {
        CertificateUse::Server => webpki::KeyUsage::server_auth(),
        CertificateUse::Client => webpki::KeyUsage::client_auth(),
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

#[cfg(test)]
mod tests;
