use std::collections::BTreeMap;

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
