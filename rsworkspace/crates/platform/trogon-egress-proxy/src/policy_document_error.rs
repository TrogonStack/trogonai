use crate::static_snapshot::SnapshotError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PolicyDocumentError {
    #[error("policy envelope shape is invalid")]
    Envelope,
    #[error("policy syntax is invalid")]
    Syntax,
    #[error("policy names an invalid agent")]
    InvalidAgent,
    #[error("policy names an invalid destination")]
    InvalidDestination,
    #[error("policy has no agents")]
    NoAgents,
    #[error("policy has an empty permit list")]
    EmptyPermitList,
    #[error("policy content is unavailable")]
    Unavailable,
}

impl From<SnapshotError> for PolicyDocumentError {
    fn from(error: SnapshotError) -> Self {
        match error {
            SnapshotError::Unparsable { .. } => Self::Syntax,
            SnapshotError::InvalidAgent { .. } => Self::InvalidAgent,
            SnapshotError::InvalidDestination { .. } => Self::InvalidDestination,
            SnapshotError::NoAgents => Self::NoAgents,
            SnapshotError::EmptyPermitList { .. } => Self::EmptyPermitList,
            SnapshotError::Unreadable { .. } => Self::Unavailable,
        }
    }
}
