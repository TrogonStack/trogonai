use std::sync::Arc;

use rustls::client::ResolvesClientCert;
use rustls::pki_types::SubjectPublicKeyInfoDer;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::{CertifiedKey, Signer, SigningKey};
use rustls::{SignatureAlgorithm, SignatureScheme};

use crate::material_authority::MaterialAuthority;

#[derive(Debug)]
pub(crate) struct LiveResolver {
    key: Arc<CertifiedKey>,
    authority: MaterialAuthority,
}

impl LiveResolver {
    pub fn new(key: &Arc<CertifiedKey>, authority: MaterialAuthority) -> Self {
        let mut guarded = CertifiedKey::new(
            key.cert.clone(),
            Arc::new(LiveSigningKey {
                key: Arc::clone(&key.key),
                authority: authority.clone(),
            }),
        );
        guarded.ocsp.clone_from(&key.ocsp);
        Self {
            key: Arc::new(guarded),
            authority,
        }
    }
}

impl ResolvesServerCert for LiveResolver {
    fn resolve(&self, _: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.authority.require_current().ok()?;
        Some(Arc::clone(&self.key))
    }
}

impl ResolvesClientCert for LiveResolver {
    fn resolve(&self, _: &[&[u8]], schemes: &[SignatureScheme]) -> Option<Arc<CertifiedKey>> {
        self.authority.require_current().ok()?;
        self.key.key.choose_scheme(schemes)?;
        Some(Arc::clone(&self.key))
    }
    fn has_certs(&self) -> bool {
        self.authority.require_current().is_ok()
    }
}

struct LiveSigningKey {
    key: Arc<dyn SigningKey>,
    authority: MaterialAuthority,
}

impl std::fmt::Debug for LiveSigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSigningKey")
            .field("algorithm", &self.key.algorithm())
            .finish_non_exhaustive()
    }
}

impl SigningKey for LiveSigningKey {
    fn choose_scheme(&self, schemes: &[SignatureScheme]) -> Option<Box<dyn Signer>> {
        self.authority.require_current().ok()?;
        let signer = self.key.choose_scheme(schemes)?;
        Some(Box::new(LiveSigner {
            signer,
            authority: self.authority.clone(),
        }))
    }
    fn public_key(&self) -> Option<SubjectPublicKeyInfoDer<'_>> {
        self.key.public_key()
    }
    fn algorithm(&self) -> SignatureAlgorithm {
        self.key.algorithm()
    }
}

struct LiveSigner {
    signer: Box<dyn Signer>,
    authority: MaterialAuthority,
}

impl std::fmt::Debug for LiveSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveSigner")
            .field("scheme", &self.signer.scheme())
            .finish_non_exhaustive()
    }
}

impl Signer for LiveSigner {
    fn sign(&self, message: &[u8]) -> Result<Vec<u8>, rustls::Error> {
        self.authority.require_current()?;
        let signature = self.signer.sign(message)?;
        self.authority.require_current()?;
        Ok(signature)
    }
    fn scheme(&self) -> SignatureScheme {
        self.signer.scheme()
    }
}

#[cfg(test)]
mod tests;
