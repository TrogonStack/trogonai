use crate::RegistrationArtifactKind;
use async_nats::jetstream::kv;
use std::time::SystemTime;

pub(super) struct RegistrationArchiveSource {
    pub(super) kind: RegistrationArtifactKind,
    pub(super) bucket: kv::Store,
    pub(super) created: SystemTime,
}
