use super::{RegistrationArchiveReceipt, RegistrationArtifactObservation};

#[derive(Debug)]
pub enum RegistrationArchiveCreate {
    Acknowledged(RegistrationArchiveReceipt),
    ExistingExact(RegistrationArtifactObservation),
}
