use std::collections::BTreeMap;
use std::sync::Arc;

use prost::Message;
use spiffe::{TrustDomain, X509Bundle, X509BundleSet, X509Context, X509Svid};

use crate::MaterialError;
use crate::constants::{MAX_CONTEXT_BYTES, MAX_IDENTITIES, MAX_ROOTS};
use crate::workload_identity_wire::WorkloadIdentityWire;

#[derive(PartialEq, prost::Message)]
#[prost(skip_debug)]
pub(crate) struct WorkloadResponseWire {
    #[prost(message, repeated, tag = "1")]
    pub svids: Vec<WorkloadIdentityWire>,
    #[prost(bytes = "vec", repeated, tag = "2")]
    pub crls: Vec<Vec<u8>>,
    #[prost(btree_map = "string, bytes", tag = "3")]
    pub federated_bundles: BTreeMap<String, Vec<u8>>,
}

impl std::fmt::Debug for WorkloadResponseWire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkloadResponseWire { <redacted> }")
    }
}

impl WorkloadResponseWire {
    pub fn into_context(self) -> Result<X509Context, MaterialError> {
        if !self.crls.is_empty() {
            return Err(MaterialError::RevocationUnsupported);
        }
        if self.encoded_len() > MAX_CONTEXT_BYTES
            || self.svids.len() > MAX_IDENTITIES
            || self.federated_bundles.len() > MAX_ROOTS
        {
            return Err(MaterialError::Bounds);
        }
        if self.svids.is_empty() {
            return Err(MaterialError::Identity);
        }
        let mut svids = Vec::with_capacity(self.svids.len());
        let mut bundles = X509BundleSet::new();
        for entry in &self.svids {
            if entry.spiffe_id.is_empty() || entry.bundle.is_empty() {
                return Err(MaterialError::Identity);
            }
            let svid =
                X509Svid::parse_from_der(&entry.x509_svid, &entry.x509_svid_key).map_err(|_| MaterialError::Profile)?;
            if svid.spiffe_id().to_string() != entry.spiffe_id {
                return Err(MaterialError::Identity);
            }
            let bundle = X509Bundle::parse_from_der(svid.spiffe_id().trust_domain().clone(), &entry.bundle)
                .map_err(|_| MaterialError::Certificate)?;
            bundles.add_bundle(bundle);
            svids.push(Arc::new(svid));
        }
        for (domain, bytes) in self.federated_bundles {
            let domain = TrustDomain::new(&domain).map_err(|_| MaterialError::Identity)?;
            let bundle = X509Bundle::parse_from_der(domain, &bytes).map_err(|_| MaterialError::Certificate)?;
            bundles.add_bundle(bundle);
        }
        Ok(X509Context::new(svids, bundles))
    }
}
