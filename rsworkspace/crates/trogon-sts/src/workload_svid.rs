use crate::spiffe_id::SpiffeId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkloadSvid {
    pub spiffe_id: SpiffeId,
    pub leaf_der: Vec<u8>,
    pub leaf_pem: String,
}

impl WorkloadSvid {
    pub fn new(spiffe_id: SpiffeId, leaf_der: Vec<u8>, leaf_pem: String) -> Self {
        Self {
            spiffe_id,
            leaf_der,
            leaf_pem,
        }
    }

    #[must_use]
    pub fn wkl(&self) -> String {
        self.spiffe_id.wkl()
    }
}
