use crate::{RegistrationArchiveError, RegistrationArtifactInput, RegistrationArtifactKind, Sha256Digest};
use bytes::Bytes;

/// Bounded original public bytes, not interpreted approval or manifest validity.
#[derive(Clone, PartialEq, Eq)]
pub struct RegistrationArtifact {
    kind: RegistrationArtifactKind,
    bytes: Bytes,
    digest: Sha256Digest,
}
impl TryFrom<RegistrationArtifactInput> for RegistrationArtifact {
    type Error = RegistrationArchiveError;
    fn try_from(input: RegistrationArtifactInput) -> Result<Self, Self::Error> {
        if input.bytes.is_empty() || input.bytes.len() > input.kind.maximum_bytes() {
            return Err(RegistrationArchiveError::ArtifactBounds);
        }
        Ok(Self {
            kind: input.kind,
            digest: Sha256Digest::hash(&input.bytes),
            bytes: input.bytes,
        })
    }
}
impl RegistrationArtifact {
    pub fn kind(&self) -> RegistrationArtifactKind {
        self.kind
    }
    pub fn bytes(&self) -> &Bytes {
        &self.bytes
    }
    pub fn digest(&self) -> Sha256Digest {
        self.digest
    }
}
impl std::fmt::Debug for RegistrationArtifact {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegistrationArtifact")
            .field("kind", &self.kind)
            .field("byte_count", &self.bytes.len())
            .finish_non_exhaustive()
    }
}
