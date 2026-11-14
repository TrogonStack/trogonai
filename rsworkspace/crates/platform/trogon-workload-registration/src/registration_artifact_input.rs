use crate::RegistrationArtifactKind;
use bytes::Bytes;

pub struct RegistrationArtifactInput {
    pub kind: RegistrationArtifactKind,
    pub bytes: Bytes,
}
