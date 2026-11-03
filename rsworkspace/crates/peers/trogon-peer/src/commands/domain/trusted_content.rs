use super::SealedContentRef;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedContent {
    pub(crate) reference: SealedContentRef,
    pub(crate) size: u32,
    pub(crate) digest: Vec<u8>,
}
impl TrustedContent {
    pub fn from_sealed_port(
        reference: SealedContentRef,
        size: u32,
        digest: Vec<u8>,
    ) -> Result<Self, super::PeerValueError> {
        if !(1..=2048).contains(&size) || digest.len() != 32 {
            return Err(super::PeerValueError::InvalidContent);
        }
        Ok(Self {
            reference,
            size,
            digest,
        })
    }
}
