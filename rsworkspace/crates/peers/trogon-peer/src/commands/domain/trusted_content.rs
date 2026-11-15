use super::{ContentSemanticFingerprint, SealedContentRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedContent {
    pub(crate) reference: SealedContentRef,
    pub(crate) size: u32,
    pub(crate) semantic_fingerprint: ContentSemanticFingerprint,
}

impl TrustedContent {
    pub fn from_sealed_port(
        reference: SealedContentRef,
        size: u32,
        semantic_fingerprint: Vec<u8>,
    ) -> Result<Self, super::PeerValueError> {
        if !(1..=2048).contains(&size) {
            return Err(super::PeerValueError::InvalidContent);
        }
        Ok(Self {
            reference,
            size,
            semantic_fingerprint: ContentSemanticFingerprint::new(semantic_fingerprint)?,
        })
    }
}
