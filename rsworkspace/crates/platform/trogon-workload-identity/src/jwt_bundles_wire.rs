use std::collections::BTreeMap;

/// `JWTBundlesResponse` of the SPIFFE Workload API: one JWKS document per
/// trust domain, keyed by the trust domain name.
#[derive(PartialEq, prost::Message)]
pub(crate) struct JwtBundlesWire {
    #[prost(btree_map = "string, bytes", tag = "1")]
    pub bundles: BTreeMap<String, Vec<u8>>,
}
