use std::sync::Arc;

use prost::Message;
use spiffe::{TrustDomain, X509Bundle, X509BundleSet, X509Svid};

use crate::MaterialError;
use crate::bundle_roots::BundleRoots;
use crate::constants::{MAX_CONTEXT_BYTES, MAX_IDENTITIES, MAX_ROOTS, MAX_SPIFFE_ID_BYTES};
use crate::workload_response_error::WorkloadResponseError;

use crate::workload_response_wire::WorkloadResponseWire;

pub(crate) struct WorkloadContext {
    svids: Vec<Arc<X509Svid>>,
    bundles: X509BundleSet,
}

impl WorkloadContext {
    pub fn from_wire(response: WorkloadResponseWire) -> Result<Self, WorkloadResponseError> {
        if !response.crls.is_empty() {
            return Err(MaterialError::RevocationUnsupported.into());
        }
        if response.encoded_len() > MAX_CONTEXT_BYTES
            || response.svids.len() > MAX_IDENTITIES
            || response.federated_bundles.len() > MAX_ROOTS
        {
            return Err(MaterialError::Bounds.into());
        }
        if response.svids.is_empty() {
            return Err(MaterialError::Identity.into());
        }
        let mut svids = Vec::with_capacity(response.svids.len());
        let mut bundles = X509BundleSet::new();
        for entry in &response.svids {
            if entry.spiffe_id.is_empty() || entry.spiffe_id.len() > MAX_SPIFFE_ID_BYTES || entry.bundle.is_empty() {
                return Err(MaterialError::Identity.into());
            }
            let svid = X509Svid::parse_from_der(&entry.x509_svid, &entry.x509_svid_key)?;
            if svid.spiffe_id().to_string() != entry.spiffe_id {
                return Err(MaterialError::Identity.into());
            }
            let bundle = X509Bundle::parse_from_der(svid.spiffe_id().trust_domain().clone(), &entry.bundle)?;
            let roots = BundleRoots::new(&bundle)?;
            if let Some(previous) = bundles.get(svid.spiffe_id().trust_domain())
                && BundleRoots::new(&previous)? != roots
            {
                return Err(MaterialError::Identity.into());
            }
            bundles.add_bundle(bundle);
            svids.push(Arc::new(svid));
        }
        for (domain, bytes) in response.federated_bundles {
            let domain = TrustDomain::new(&domain)?;
            if bundles.get(&domain).is_some() {
                return Err(MaterialError::Identity.into());
            }
            let bundle = X509Bundle::parse_from_der(domain, &bytes)?;
            BundleRoots::new(&bundle)?;
            bundles.add_bundle(bundle);
        }
        Ok(Self { svids, bundles })
    }

    pub fn svids(&self) -> &[Arc<X509Svid>] {
        &self.svids
    }

    pub fn bundle_set(&self) -> &X509BundleSet {
        &self.bundles
    }
}
