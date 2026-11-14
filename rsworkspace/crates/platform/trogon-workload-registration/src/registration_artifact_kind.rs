use crate::constants::{APPROVAL_WRAPPER_FRAMING_BYTES, MAX_NATIVE_HEADER_BYTES};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationArtifactKind {
    OriginalApproval,
    BootstrapManifest,
}
impl RegistrationArtifactKind {
    pub const ALL: [Self; 2] = [Self::OriginalApproval, Self::BootstrapManifest];
    pub fn maximum_bytes(self) -> usize {
        match self {
            Self::OriginalApproval => crate::MAX_SNAPSHOT_BYTES + APPROVAL_WRAPPER_FRAMING_BYTES,
            Self::BootstrapManifest => 64 * 1024,
        }
    }
    pub fn maximum_message_bytes(self) -> usize {
        self.maximum_bytes() + MAX_NATIVE_HEADER_BYTES
    }
}
