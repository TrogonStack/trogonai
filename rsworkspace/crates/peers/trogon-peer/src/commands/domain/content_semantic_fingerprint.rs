use super::PeerValueError;
use trogonai_proto::peers::v1alpha1 as proto;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentSemanticFingerprint([u8; 32]);

impl ContentSemanticFingerprint {
    pub fn new(value: Vec<u8>) -> Result<Self, PeerValueError> {
        value
            .try_into()
            .map(Self)
            .map_err(|_| PeerValueError::InvalidSemanticFingerprint)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_wire(&self) -> proto::ContentSemanticFingerprint {
        proto::ContentSemanticFingerprint {
            value: Some(self.0.to_vec()),
        }
    }
}
