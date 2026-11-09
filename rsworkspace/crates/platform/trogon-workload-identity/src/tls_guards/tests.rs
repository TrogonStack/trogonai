use super::*;
use crate::material_authority::MaterialStatus;

#[derive(Debug)]
struct Withdrawn;

impl MaterialStatus for Withdrawn {
    fn is_current(&self) -> bool {
        false
    }
}

struct KeyCanary;

impl std::fmt::Debug for KeyCanary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("provider-private-material-canary")
    }
}

impl SigningKey for KeyCanary {
    fn choose_scheme(&self, _: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        None
    }
    fn algorithm(&self) -> SignatureAlgorithm {
        SignatureAlgorithm::ECDSA
    }
}

struct SignerCanary;

impl std::fmt::Debug for SignerCanary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("provider-private-material-canary")
    }
}

impl Signer for SignerCanary {
    fn sign(&self, _: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        Err(rustls::Error::InvalidCertificate(
            rustls::CertificateError::ApplicationVerificationFailure,
        ))
    }
    fn scheme(&self) -> SignatureScheme {
        SignatureScheme::ECDSA_NISTP256_SHA256
    }
}

#[test]
fn tls_wrapper_diagnostics_do_not_forward_private_provider_debug_output() {
    let authority = MaterialAuthority::new(Arc::new(Withdrawn));
    let key = LiveSigningKey {
        key: Arc::new(KeyCanary),
        authority: authority.clone(),
    };
    let signer = LiveSigner {
        signer: Box::new(SignerCanary),
        authority,
    };
    assert!(!format!("{key:?}").contains("provider-private-material-canary"));
    assert!(!format!("{signer:?}").contains("provider-private-material-canary"));
    assert!(signer.sign(b"withdrawn").is_err());
}
