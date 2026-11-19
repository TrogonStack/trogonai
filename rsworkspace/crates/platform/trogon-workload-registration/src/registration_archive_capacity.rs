use crate::{RegistrationArchiveError, RegistrationArtifactKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationArchiveCapacity {
    kind: RegistrationArtifactKind,
    records: i64,
    bytes: i64,
}
impl RegistrationArchiveCapacity {
    pub fn new(kind: RegistrationArtifactKind, records: u64, bytes: u64) -> Result<Self, RegistrationArchiveError> {
        let records = i64::try_from(records).map_err(|_| RegistrationArchiveError::Capacity)?;
        let bytes = i64::try_from(bytes).map_err(|_| RegistrationArchiveError::Capacity)?;
        if records <= 0 || bytes < kind.maximum_message_bytes() as i64 {
            return Err(RegistrationArchiveError::Capacity);
        }
        Ok(Self { kind, records, bytes })
    }
    pub fn kind(self) -> RegistrationArtifactKind {
        self.kind
    }
    pub fn records(self) -> i64 {
        self.records
    }
    pub fn bytes(self) -> i64 {
        self.bytes
    }
}
