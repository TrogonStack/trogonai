use async_nats::jetstream::context::CreateKeyValueError;
use async_nats::jetstream::kv;

use crate::{DaytonaApiError, DaytonaOrganizationIdError};

/// Why snapshot custody refused a capture or could not finish a cleanup. Every refusal keeps the custody it had.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotCustodyError {
    #[error("snapshot custody bucket is unavailable")]
    Bucket(#[from] CreateKeyValueError),
    #[error("invalid snapshot custody identity")]
    InvalidIdentity,
    #[error("invalid snapshot custody state")]
    InvalidState,
    #[error("invalid native snapshot name")]
    InvalidName,
    #[error("snapshot custody record exceeds its limit")]
    TooLarge,
    #[error("snapshot custody record could not be encoded")]
    Encode(#[source] serde_json::Error),
    #[error("invalid snapshot custody record")]
    Decode(#[source] serde_json::Error),
    #[error("snapshot custody was removed")]
    Removed,
    #[error("snapshot custody record could not be created")]
    Create(#[from] kv::CreateError),
    #[error("snapshot custody record could not be updated")]
    Update(#[from] kv::UpdateError),
    #[error("snapshot custody record could not be read")]
    Read(#[from] kv::EntryError),
    #[error(transparent)]
    Api(#[from] DaytonaApiError),
    #[error("the retained organization is not usable")]
    Organization(#[from] DaytonaOrganizationIdError),
    #[error("snapshot custody is missing")]
    Missing,
    #[error("the source sandbox identity is unconfirmed")]
    SourceUnconfirmed,
    #[error("the source sandbox is closed or its account changed")]
    SourceClosedOrMoved,
    #[error("the capture name is already reserved or the source has no capacity left")]
    CaptureReserved,
    #[error("the snapshot name already exists, so no create authority was earned")]
    NameExists,
    #[error("the snapshot intent is missing")]
    IntentMissing,
    #[error("the create response names a foreign source or account")]
    ForeignCreate,
    #[error("the create acknowledgement has no eligible intent")]
    NotEligible,
    #[error("the create acknowledgement could not be persisted")]
    AcknowledgementLost,
    #[error("the native snapshot ownership binding changed")]
    OwnershipChanged,
    #[error("snapshot cleanup or publication already started")]
    AlreadyStarted,
    #[error("the snapshot binding could not be persisted after bounded retries")]
    BindingLost,
    #[error("the closed source refused publication; exact cleanup custody is retained")]
    PublicationRefused,
    #[error("the source close fence could not be persisted")]
    FenceLost,
    #[error("the source sandbox does not match the retained cleanup custody")]
    SourceMismatch,
    #[error("an unbound capture remains quarantined, so source cleanup is unresolved")]
    Quarantined,
    #[error("the acknowledged snapshot is not yet visible, so exact cleanup remains unresolved")]
    NotYetVisible,
    #[error("the late snapshot binding changed before exact cleanup")]
    LateBindingChanged,
    #[error("the snapshot deletion has no exact absence confirmation")]
    DeletionUnconfirmed,
}
