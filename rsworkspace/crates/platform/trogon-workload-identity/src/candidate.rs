use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};
use spiffe::{SpiffeId, X509Context};
use tokio::time::Instant;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::constants::{MAX_CONTEXT_BYTES, MAX_IDENTITIES, MAX_ROOTS};
use crate::material_error::MaterialError;
use crate::certificate_validation::{leaf_profile, verify_chain};

pub(crate) struct Candidate {
    pub certified_key: Arc<CertifiedKey>,
    pub roots: Vec<CertificateDer<'static>>,
    pub root_hashes: Vec<[u8; 32]>,
    pub leaf_sha256: [u8; 32],
    pub public_key_sha256: [u8; 32],
    pub bundle_sha256: [u8; 32],
    pub not_before: SystemTime,
    pub expires_at: SystemTime,
    pub deadline: Instant,
}

impl Candidate {
    pub fn parse(context: X509Context, expected: &SpiffeId) -> Result<Self, MaterialError> {
        if context.svids().len() > MAX_IDENTITIES || context.bundle_set().len() > MAX_ROOTS {
            return Err(MaterialError::Bounds);
        }
        let mut bytes = 0usize;
        let mut ids = BTreeSet::new();
        for svid in context.svids() {
            if !ids.insert(svid.spiffe_id()) {
                return Err(MaterialError::Identity);
            }
            bytes = bytes.saturating_add(svid.private_key().as_bytes().len());
            for certificate in svid.cert_chain() {
                bytes = bytes.saturating_add(certificate.as_bytes().len());
            }
        }
        for (_, bundle) in context.bundle_set().iter() {
            if bundle.authorities().len() > MAX_ROOTS {
                return Err(MaterialError::Bounds);
            }
            for root in bundle.authorities() {
                bytes = bytes.saturating_add(root.as_bytes().len());
            }
        }
        if bytes > MAX_CONTEXT_BYTES {
            return Err(MaterialError::Bounds);
        }
        let svid = context
            .svids()
            .iter()
            .find(|svid| svid.spiffe_id() == expected)
            .ok_or(MaterialError::Identity)?;
        let bundle = context
            .bundle_set()
            .get(expected.trust_domain())
            .ok_or(MaterialError::Identity)?;
        if bundle.authorities().is_empty() {
            return Err(MaterialError::Identity);
        }
        let roots: Vec<_> = bundle
            .authorities()
            .iter()
            .map(|root| CertificateDer::from(root.as_bytes().to_vec()))
            .collect();
        let mut root_hashes: Vec<[u8; 32]> = roots.iter().map(|root| Sha256::digest(root).into()).collect();
        root_hashes.sort_unstable();
        if root_hashes.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(MaterialError::Identity);
        }
        let mut hash = Sha256::new();
        for root in &root_hashes {
            hash.update(root);
        }
        let bundle_sha256 = hash.finalize().into();
        let chain: Vec<_> = svid
            .cert_chain()
            .iter()
            .map(|certificate| CertificateDer::from(certificate.as_bytes().to_vec()))
            .collect();
        let observation = crate::time_observation::TimeObservation::now();
        let now = observation.wall();
        let leaf = chain.first().ok_or(MaterialError::Identity)?;
        let (_, not_before, _) = leaf_profile(leaf, now)?;
        let expires_at = verify_chain(&chain, &roots, now, false)?.min(verify_chain(&chain, &roots, now, true)?);
        let (_, parsed) = X509Certificate::from_der(leaf).map_err(|_| MaterialError::Profile)?;
        let public_key_sha256 = Sha256::digest(parsed.public_key().raw).into();
        let leaf_sha256 = Sha256::digest(leaf).into();
        let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(svid.private_key().as_bytes().to_vec()));
        let provider = rustls::crypto::aws_lc_rs::default_provider();
        let certified_key = CertifiedKey::from_der(chain, key, &provider).map_err(|_| MaterialError::Key)?;
        certified_key.keys_match().map_err(|_| MaterialError::Key)?;
        let deadline = observation.deadline(expires_at).map_err(|_| MaterialError::Profile)?;
        Ok(Self {
            certified_key: Arc::new(certified_key),
            roots,
            root_hashes,
            leaf_sha256,
            public_key_sha256,
            bundle_sha256,
            not_before,
            expires_at,
            deadline,
        })
    }

    pub fn current(&self) -> bool {
        let now = SystemTime::now();
        self.not_before <= now && self.expires_at > now && self.deadline > Instant::now()
    }
}

