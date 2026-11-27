use super::*;

/// PEM-encoded test fixture (the same `fence-test-ca` the TS-side
/// tls-terminate tests use). `from_pem_or_der` parses it to a
/// real `CERT_CONTEXT`, so the canonicalization step is
/// exercised end-to-end.
const CA_PEM: &[u8] = include_bytes!("../../tests/data/tls-terminate-ca.crt");

#[test]
fn pem_roundtrip_is_canonical() -> Result<(), Box<dyn std::error::Error>> {
    let c = CertDer::from_pem_or_der(CA_PEM)?;
    // Re-encode → re-decode is byte-identical.
    let pem = c.to_pem()?;
    assert!(pem.contains("BEGIN CERTIFICATE"));
    assert_eq!(CertDer::from_pem_or_der(pem.as_bytes())?.as_bytes(), c.as_bytes());
    // 40-hex-char SHA-1 thumb.
    assert_eq!(c.thumb()?.len(), 40);
    Ok(())
}

#[test]
fn der_with_trailing_bytes_canonicalizes() -> Result<(), Box<dyn std::error::Error>> {
    let canon = CertDer::from_pem_or_der(CA_PEM)?;
    // DER input + junk past the outer SEQUENCE → same canonical
    // bytes, same thumb.
    let mut dirty = canon.as_bytes().to_vec();
    dirty.extend_from_slice(b"\x00trailing junk");
    let reparsed = CertDer::from_pem_or_der(&dirty)?;
    assert_eq!(reparsed.as_bytes(), canon.as_bytes());
    assert_eq!(reparsed.thumb()?, canon.thumb()?);
    Ok(())
}

#[test]
fn rejects_non_x509() {
    // SEQUENCE-tagged but not a cert → CertCreateCertificateContext
    // fails and so does the constructor.
    assert!(CertDer::from_pem_or_der(b"\x30\x03\x02\x01\x05").is_err());
    // PEM-wrapped non-cert payload → same.
    let pem = b"-----BEGIN CERTIFICATE-----\r\naGVsbG8=\r\n-----END CERTIFICATE-----\r\n";
    assert!(CertDer::from_pem_or_der(pem).is_err());
}
