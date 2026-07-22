#![allow(clippy::all)]

mod artifact_id;
mod correlation_id;
mod event_id;
mod idempotency_key;
mod identifier;
mod operation_id;
mod seq;
mod session_id;
mod tool_execution_id;

mod proto {
    include!(concat!(env!("OUT_DIR"), "/_include.rs"));
}

pub use artifact_id::ArtifactId;
pub use correlation_id::CorrelationId;
pub use event_id::EventId;
pub use idempotency_key::IdempotencyKey;
pub use identifier::IdentifierError;
pub use operation_id::OperationId;
pub use seq::{Seq, SeqError};
pub use session_id::SessionId;
pub use tool_execution_id::ToolExecutionId;

pub use proto::trogonai::session::v1::*;

/// Current schema version for all durable session contracts in this crate.
pub const SCHEMA_VERSION_V1: u32 = 1;

/// Error returned when a generated contract fails domain validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContractValidationError {
    UnsupportedSchemaVersion { expected: u32, actual: u32 },
    InvalidSessionId(IdentifierError),
    InvalidEventId(IdentifierError),
    InvalidOperationId(IdentifierError),
    InvalidCorrelationId(IdentifierError),
    InvalidIdempotencyKey(IdentifierError),
    InvalidArtifactId(IdentifierError),
    InvalidSeq(SeqError),
    MissingField(&'static str),
}

impl std::fmt::Display for ContractValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSchemaVersion { expected, actual } => {
                write!(
                    f,
                    "unsupported schema version: expected v{expected}, got v{actual}"
                )
            }
            Self::InvalidSessionId(err) => write!(f, "invalid session_id: {err}"),
            Self::InvalidEventId(err) => write!(f, "invalid event_id: {err}"),
            Self::InvalidOperationId(err) => write!(f, "invalid operation_id: {err}"),
            Self::InvalidCorrelationId(err) => write!(f, "invalid correlation_id: {err}"),
            Self::InvalidIdempotencyKey(err) => write!(f, "invalid idempotency_key: {err}"),
            Self::InvalidArtifactId(err) => write!(f, "invalid artifact_id: {err}"),
            Self::InvalidSeq(err) => write!(f, "invalid seq: {err}"),
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
        }
    }
}

impl std::error::Error for ContractValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidSessionId(err)
            | Self::InvalidEventId(err)
            | Self::InvalidOperationId(err)
            | Self::InvalidCorrelationId(err)
            | Self::InvalidIdempotencyKey(err)
            | Self::InvalidArtifactId(err) => Some(err),
            Self::InvalidSeq(err) => Some(err),
            _ => None,
        }
    }
}

/// Validated view of a [`SessionEvent`] contract.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedSessionEvent {
    pub schema_version: u32,
    pub event_id: EventId,
    pub session_id: SessionId,
    pub seq: Seq,
    pub operation_id: OperationId,
    pub correlation_id: CorrelationId,
    pub idempotency_key: IdempotencyKey,
    pub event: SessionEvent,
}

impl ValidatedSessionEvent {
    pub fn try_from_event(event: SessionEvent) -> Result<Self, ContractValidationError> {
        if event.schema_version != 0 && event.schema_version != SCHEMA_VERSION_V1 {
            return Err(ContractValidationError::UnsupportedSchemaVersion {
                expected: SCHEMA_VERSION_V1,
                actual: event.schema_version,
            });
        }
        if event.event_id.is_empty() {
            return Err(ContractValidationError::MissingField("event_id"));
        }
        if event.session_id.is_empty() {
            return Err(ContractValidationError::MissingField("session_id"));
        }
        if event.operation_id.is_empty() {
            return Err(ContractValidationError::MissingField("operation_id"));
        }
        if event.correlation_id.is_empty() {
            return Err(ContractValidationError::MissingField("correlation_id"));
        }
        if event.idempotency_key.is_empty() {
            return Err(ContractValidationError::MissingField("idempotency_key"));
        }
        // NOTE: `actor` and `payload` are listed obligatorio by §4, but are
        // intentionally NOT enforced here: the §Schema Versioning N-1 rule requires
        // accepting minimal/older events that may lack them (see the
        // `n_minus_one_reader_accepts_minimal_event_without_optional_fields` contract
        // test). The two doc requirements conflict; N-1 compatibility wins.

        Ok(Self {
            schema_version: if event.schema_version == 0 {
                SCHEMA_VERSION_V1
            } else {
                event.schema_version
            },
            event_id: EventId::new(&event.event_id)
                .map_err(ContractValidationError::InvalidEventId)?,
            session_id: SessionId::new(&event.session_id)
                .map_err(ContractValidationError::InvalidSessionId)?,
            seq: Seq::new(event.seq).map_err(ContractValidationError::InvalidSeq)?,
            operation_id: OperationId::new(&event.operation_id)
                .map_err(ContractValidationError::InvalidOperationId)?,
            correlation_id: CorrelationId::new(&event.correlation_id)
                .map_err(ContractValidationError::InvalidCorrelationId)?,
            idempotency_key: IdempotencyKey::new(&event.idempotency_key)
                .map_err(ContractValidationError::InvalidIdempotencyKey)?,
            event,
        })
    }
}

/// Validated view of a [`SessionSnapshot`] contract.
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedSessionSnapshot {
    pub schema_version: u32,
    pub session_id: SessionId,
    pub last_applied_seq: Seq,
    pub snapshot: SessionSnapshot,
}

impl ValidatedSessionSnapshot {
    pub fn try_from_snapshot(snapshot: SessionSnapshot) -> Result<Self, ContractValidationError> {
        if snapshot.schema_version != 0 && snapshot.schema_version != SCHEMA_VERSION_V1 {
            return Err(ContractValidationError::UnsupportedSchemaVersion {
                expected: SCHEMA_VERSION_V1,
                actual: snapshot.schema_version,
            });
        }
        if snapshot.session_id.is_empty() {
            return Err(ContractValidationError::MissingField("session_id"));
        }

        Ok(Self {
            schema_version: if snapshot.schema_version == 0 {
                SCHEMA_VERSION_V1
            } else {
                snapshot.schema_version
            },
            session_id: SessionId::new(&snapshot.session_id)
                .map_err(ContractValidationError::InvalidSessionId)?,
            last_applied_seq: Seq::new(snapshot.last_applied_seq)
                .map_err(ContractValidationError::InvalidSeq)?,
            snapshot,
        })
    }
}

/// Validated view of [`ArtifactMetadata`].
#[derive(Clone, Debug, PartialEq)]
pub struct ValidatedArtifactMetadata {
    pub schema_version: u32,
    pub artifact_id: ArtifactId,
    pub session_id: SessionId,
    pub event_id: EventId,
    pub metadata: ArtifactMetadata,
}

impl ValidatedArtifactMetadata {
    pub fn try_from_metadata(metadata: ArtifactMetadata) -> Result<Self, ContractValidationError> {
        if metadata.schema_version != 0 && metadata.schema_version != SCHEMA_VERSION_V1 {
            return Err(ContractValidationError::UnsupportedSchemaVersion {
                expected: SCHEMA_VERSION_V1,
                actual: metadata.schema_version,
            });
        }
        if metadata.artifact_id.is_empty() {
            return Err(ContractValidationError::MissingField("artifact_id"));
        }
        if metadata.session_id.is_empty() {
            return Err(ContractValidationError::MissingField("session_id"));
        }
        if metadata.event_id.is_empty() {
            return Err(ContractValidationError::MissingField("event_id"));
        }

        Ok(Self {
            schema_version: if metadata.schema_version == 0 {
                SCHEMA_VERSION_V1
            } else {
                metadata.schema_version
            },
            artifact_id: ArtifactId::new(&metadata.artifact_id)
                .map_err(ContractValidationError::InvalidArtifactId)?,
            session_id: SessionId::new(&metadata.session_id)
                .map_err(ContractValidationError::InvalidSessionId)?,
            event_id: EventId::new(&metadata.event_id)
                .map_err(ContractValidationError::InvalidEventId)?,
            metadata,
        })
    }
}
