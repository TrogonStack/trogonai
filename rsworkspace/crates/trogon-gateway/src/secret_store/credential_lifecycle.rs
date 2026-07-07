use std::convert::Infallible;
use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;

use buffa::{EnumValue, Message as _, MessageField};
use trogon_decider_runtime::{
    CommandSnapshotPolicy, Decider, Decision, EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity,
    EventType, FrequencySnapshot, InvalidSnapshotTypeName, SnapshotPayloadData, SnapshotPayloadDecode,
    SnapshotPayloadEncode, SnapshotType, SnapshotTypeName, WritePrecondition,
};
use trogonai_proto::gateway::credentials::v1 as proto;
use trogonai_proto::gateway::credentials::v1::__buffa::oneof::credential_lifecycle_state_snapshot::State as CredentialLifecycleStateSnapshotCase;

use super::{
    CredentialFingerprint, CredentialId, CredentialKind, CredentialMetadata, CredentialOwnerId, CredentialRef,
    CredentialScope, CredentialStatus, CredentialVersion, SourceKind, StorageBackend,
};
use crate::source_integration_id::SourceIntegrationId;

const CREDENTIAL_LIFECYCLE_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();
pub(crate) const CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY: FrequencySnapshot =
    FrequencySnapshot::new(CREDENTIAL_LIFECYCLE_SNAPSHOT_EVERY);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialLifecycleState {
    Missing,
    PendingWrite(PendingCredentialWrite),
    Active(ActiveCredential),
    WriteFailed(FailedCredentialWrite),
    RotationPending(RotationPendingCredential),
    Revoked(RevokedCredential),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingCredentialWrite {
    credential_id: CredentialId,
    owner_id: CredentialOwnerId,
    source: SourceKind,
    kind: CredentialKind,
}

impl PendingCredentialWrite {
    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveCredential {
    metadata: CredentialMetadata,
    previous_versions: Vec<CredentialRef>,
}

impl ActiveCredential {
    pub fn metadata(&self) -> &CredentialMetadata {
        &self.metadata
    }

    pub fn credential_ref(&self) -> &CredentialRef {
        self.metadata.reference()
    }

    pub fn previous_versions(&self) -> &[CredentialRef] {
        &self.previous_versions
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FailedCredentialWrite {
    credential_id: CredentialId,
    reason: CredentialFailureReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RotationPendingCredential {
    active: ActiveCredential,
}

impl RotationPendingCredential {
    pub fn active(&self) -> &ActiveCredential {
        &self.active
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevokedCredential {
    credential_ref: CredentialRef,
}

impl RevokedCredential {
    pub fn credential_ref(&self) -> &CredentialRef {
        &self.credential_ref
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CredentialLifecycleEvent {
    WriteRequested {
        credential_id: CredentialId,
        owner_id: CredentialOwnerId,
        source: SourceKind,
        kind: CredentialKind,
    },
    WriteFailed {
        credential_id: CredentialId,
        reason: CredentialFailureReason,
    },
    Activated {
        metadata: CredentialMetadata,
    },
    RotationRequested {
        credential_ref: CredentialRef,
    },
    RotationFailed {
        credential_ref: CredentialRef,
        reason: CredentialFailureReason,
    },
    Rotated {
        previous_credential_ref: CredentialRef,
        metadata: CredentialMetadata,
    },
    Revoked {
        credential_ref: CredentialRef,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialWrite {
    credential_id: CredentialId,
    owner_id: CredentialOwnerId,
    source: SourceKind,
    kind: CredentialKind,
}

impl RequestCredentialWrite {
    pub fn new(
        credential_id: CredentialId,
        owner_id: CredentialOwnerId,
        source: SourceKind,
        kind: CredentialKind,
    ) -> Self {
        Self {
            credential_id,
            owner_id,
            source,
            kind,
        }
    }

    pub fn credential_id(&self) -> &CredentialId {
        &self.credential_id
    }

    pub fn owner_id(&self) -> &CredentialOwnerId {
        &self.owner_id
    }

    pub fn source(&self) -> SourceKind {
        self.source
    }

    pub fn kind(&self) -> CredentialKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivateCredentialWrite {
    metadata: CredentialMetadata,
}

impl ActivateCredentialWrite {
    pub fn new(metadata: CredentialMetadata) -> Self {
        Self { metadata }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordCredentialWriteFailure {
    credential_id: CredentialId,
    reason: CredentialFailureReason,
}

impl RecordCredentialWriteFailure {
    pub fn new(credential_id: CredentialId, reason: CredentialFailureReason) -> Self {
        Self { credential_id, reason }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestCredentialRotation {
    credential_ref: CredentialRef,
}

impl RequestCredentialRotation {
    pub fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }

    pub fn credential_ref(&self) -> &CredentialRef {
        &self.credential_ref
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordCredentialRotationFailure {
    credential_ref: CredentialRef,
    reason: CredentialFailureReason,
}

impl RecordCredentialRotationFailure {
    pub fn new(credential_ref: CredentialRef, reason: CredentialFailureReason) -> Self {
        Self { credential_ref, reason }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivateCredentialRotation {
    metadata: CredentialMetadata,
}

impl ActivateCredentialRotation {
    pub fn new(metadata: CredentialMetadata) -> Self {
        Self { metadata }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevokeCredential {
    credential_ref: CredentialRef,
}

impl RevokeCredential {
    pub fn new(credential_ref: CredentialRef) -> Self {
        Self { credential_ref }
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct CredentialFailureReason(Arc<str>);

impl CredentialFailureReason {
    pub fn new(value: impl AsRef<str>) -> Result<Self, CredentialFailureReasonError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(CredentialFailureReasonError::Empty);
        }
        if value.chars().count() > 512 {
            return Err(CredentialFailureReasonError::TooLong);
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("CredentialFailureReason").field(&self.as_str()).finish()
    }
}

impl fmt::Display for CredentialFailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialFailureReasonError {
    #[error("credential failure reason must not be empty")]
    Empty,
    #[error("credential failure reason exceeds maximum length")]
    TooLong,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialLifecycleDecideError {
    #[error("credential '{credential_id}' already exists")]
    AlreadyExists { credential_id: CredentialId },
    #[error("credential '{credential_id}' was revoked")]
    Revoked { credential_id: CredentialId },
    #[error("credential '{credential_id}' does not exist")]
    CredentialNotFound { credential_id: CredentialId },
    #[error("credential '{credential_id}' write is not pending")]
    CredentialWriteNotPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' is not active")]
    CredentialNotActive { credential_id: CredentialId },
    #[error("credential '{credential_id}' rotation is already pending")]
    CredentialRotationAlreadyPending { credential_id: CredentialId },
    #[error("credential '{credential_id}' rotation is not pending")]
    CredentialRotationNotPending { credential_id: CredentialId },
    #[error("credential ref does not match lifecycle stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CredentialLifecycleEvolveError {
    #[error("credential write was requested after lifecycle already started")]
    WriteRequestedAfterStart,
    #[error("credential write failure was recorded without a pending write")]
    WriteFailedWithoutPendingWrite,
    #[error("credential was activated without a pending write")]
    ActivatedWithoutPendingWrite,
    #[error("credential rotation was requested without an active credential")]
    RotationRequestedWithoutActiveCredential,
    #[error("credential rotation failure was recorded without a pending rotation")]
    RotationFailedWithoutPendingRotation,
    #[error("credential was rotated without a pending rotation")]
    RotatedWithoutPendingRotation,
    #[error("credential was revoked without an active credential")]
    RevokedWithoutActiveCredential,
    #[error("event credential ref does not match lifecycle stream: expected '{expected}', got '{actual}'")]
    CredentialRefMismatch {
        expected: CredentialId,
        actual: CredentialId,
    },
    #[error("event credential metadata must describe an active credential: {status}")]
    MetadataNotActive { status: CredentialStatus },
    #[error("rotated credential version must be newer than current version")]
    RotationVersionNotNewer,
}

#[derive(Debug, thiserror::Error)]
pub enum CredentialLifecycleEventPayloadError {
    #[error("credential lifecycle event payload protobuf failed: {0}")]
    Decode(#[source] buffa::DecodeError),
    #[error("credential lifecycle event field '{field}' is missing")]
    MissingField { field: &'static str },
    #[error("credential lifecycle event field '{field}' is invalid: {reason}")]
    InvalidField { field: &'static str, reason: String },
}

impl EventIdentity for CredentialLifecycleEvent {}

impl EventType for CredentialLifecycleEvent {
    type Error = Infallible;

    fn event_type(&self) -> Result<&'static str, Self::Error> {
        Ok(match self {
            Self::WriteRequested { .. } => proto_event_type::<proto::CredentialWriteRequested>(),
            Self::WriteFailed { .. } => proto_event_type::<proto::CredentialWriteFailed>(),
            Self::Activated { .. } => proto_event_type::<proto::CredentialActivated>(),
            Self::RotationRequested { .. } => proto_event_type::<proto::CredentialRotationRequested>(),
            Self::RotationFailed { .. } => proto_event_type::<proto::CredentialRotationFailed>(),
            Self::Rotated { .. } => proto_event_type::<proto::CredentialRotated>(),
            Self::Revoked { .. } => proto_event_type::<proto::CredentialRevoked>(),
        })
    }
}

impl EventEncode for CredentialLifecycleEvent {
    type Error = Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        let encoded = match self {
            Self::WriteRequested {
                credential_id,
                owner_id,
                source,
                kind,
            } => proto::CredentialWriteRequested {
                credential_id: credential_id.as_str().to_string(),
                owner_id: owner_id.as_str().to_string(),
                source: Some(proto_source_kind(*source).into()),
                kind: Some(proto_credential_kind(*kind).into()),
            }
            .encode_to_vec(),
            Self::WriteFailed { credential_id, reason } => proto::CredentialWriteFailed {
                credential_id: credential_id.as_str().to_string(),
                reason: reason.as_str().to_string(),
            }
            .encode_to_vec(),
            Self::Activated { metadata } => proto::CredentialActivated {
                metadata: MessageField::some(credential_metadata_to_proto(metadata)),
            }
            .encode_to_vec(),
            Self::RotationRequested { credential_ref } => proto::CredentialRotationRequested {
                credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
            }
            .encode_to_vec(),
            Self::RotationFailed { credential_ref, reason } => proto::CredentialRotationFailed {
                credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
                reason: reason.as_str().to_string(),
            }
            .encode_to_vec(),
            Self::Rotated {
                previous_credential_ref,
                metadata,
            } => proto::CredentialRotated {
                previous_credential_ref: MessageField::some(credential_ref_to_proto(previous_credential_ref)),
                metadata: MessageField::some(credential_metadata_to_proto(metadata)),
            }
            .encode_to_vec(),
            Self::Revoked { credential_ref } => proto::CredentialRevoked {
                credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
            }
            .encode_to_vec(),
        };
        Ok(encoded)
    }
}

impl EventDecode for CredentialLifecycleEvent {
    type Error = CredentialLifecycleEventPayloadError;

    fn decode(event: EventData<'_>) -> Result<EventDecodeOutcome<Self>, Self::Error> {
        let decoded = if event.event_type == proto_event_type::<proto::CredentialWriteRequested>() {
            let payload = decode_payload::<proto::CredentialWriteRequested>(event.payload)?;
            Self::WriteRequested {
                credential_id: decode_credential_id("credential_id", &payload.credential_id)?,
                owner_id: decode_owner_id("owner_id", &payload.owner_id)?,
                source: decode_source_kind("source", payload.source.as_ref())?,
                kind: decode_credential_kind("kind", payload.kind.as_ref())?,
            }
        } else if event.event_type == proto_event_type::<proto::CredentialWriteFailed>() {
            let payload = decode_payload::<proto::CredentialWriteFailed>(event.payload)?;
            Self::WriteFailed {
                credential_id: decode_credential_id("credential_id", &payload.credential_id)?,
                reason: CredentialFailureReason::new(&payload.reason)
                    .map_err(|source| invalid_field("reason", source))?,
            }
        } else if event.event_type == proto_event_type::<proto::CredentialActivated>() {
            let payload = decode_payload::<proto::CredentialActivated>(event.payload)?;
            Self::Activated {
                metadata: decode_credential_metadata("metadata", decode_message_field("metadata", &payload.metadata)?)?,
            }
        } else if event.event_type == proto_event_type::<proto::CredentialRotationRequested>() {
            let payload = decode_payload::<proto::CredentialRotationRequested>(event.payload)?;
            Self::RotationRequested {
                credential_ref: decode_credential_ref(
                    "credential_ref",
                    decode_message_field("credential_ref", &payload.credential_ref)?,
                )?,
            }
        } else if event.event_type == proto_event_type::<proto::CredentialRotationFailed>() {
            let payload = decode_payload::<proto::CredentialRotationFailed>(event.payload)?;
            Self::RotationFailed {
                credential_ref: decode_credential_ref(
                    "credential_ref",
                    decode_message_field("credential_ref", &payload.credential_ref)?,
                )?,
                reason: CredentialFailureReason::new(&payload.reason)
                    .map_err(|source| invalid_field("reason", source))?,
            }
        } else if event.event_type == proto_event_type::<proto::CredentialRotated>() {
            let payload = decode_payload::<proto::CredentialRotated>(event.payload)?;
            Self::Rotated {
                previous_credential_ref: decode_credential_ref(
                    "previous_credential_ref",
                    decode_message_field("previous_credential_ref", &payload.previous_credential_ref)?,
                )?,
                metadata: decode_credential_metadata("metadata", decode_message_field("metadata", &payload.metadata)?)?,
            }
        } else if event.event_type == proto_event_type::<proto::CredentialRevoked>() {
            let payload = decode_payload::<proto::CredentialRevoked>(event.payload)?;
            Self::Revoked {
                credential_ref: decode_credential_ref(
                    "credential_ref",
                    decode_message_field("credential_ref", &payload.credential_ref)?,
                )?,
            }
        } else {
            return Ok(EventDecodeOutcome::Skipped);
        };
        Ok(EventDecodeOutcome::Decoded(decoded))
    }
}

impl SnapshotType for CredentialLifecycleState {
    type Error = InvalidSnapshotTypeName;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new(<proto::CredentialLifecycleStateSnapshot as buffa::MessageName>::FULL_NAME)
    }
}

impl SnapshotPayloadEncode for CredentialLifecycleState {
    type Error = Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(credential_lifecycle_state_to_proto(self).encode_to_vec())
    }
}

impl SnapshotPayloadDecode for CredentialLifecycleState {
    type Error = CredentialLifecycleEventPayloadError;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        decode_payload::<proto::CredentialLifecycleStateSnapshot>(payload.payload)
            .and_then(|snapshot| credential_lifecycle_state_from_proto("state", &snapshot))
    }
}

fn credential_lifecycle_state_to_proto(value: &CredentialLifecycleState) -> proto::CredentialLifecycleStateSnapshot {
    let state = match value {
        CredentialLifecycleState::Missing => proto::CredentialLifecycleMissingState::default().into(),
        CredentialLifecycleState::PendingWrite(pending) => proto::PendingCredentialWriteState {
            credential_id: pending.credential_id().as_str().to_string(),
            owner_id: pending.owner_id().as_str().to_string(),
            source: Some(proto_source_kind(pending.source()).into()),
            kind: Some(proto_credential_kind(pending.kind()).into()),
        }
        .into(),
        CredentialLifecycleState::Active(active) => active_state_to_proto(active).into(),
        CredentialLifecycleState::WriteFailed(failed) => proto::FailedCredentialWriteState {
            credential_id: failed.credential_id.as_str().to_string(),
            reason: failed.reason.as_str().to_string(),
        }
        .into(),
        CredentialLifecycleState::RotationPending(rotation) => proto::RotationPendingCredentialState {
            active: MessageField::some(active_state_to_proto(rotation.active())),
        }
        .into(),
        CredentialLifecycleState::Revoked(revoked) => proto::RevokedCredentialState {
            credential_ref: MessageField::some(credential_ref_to_proto(revoked.credential_ref())),
        }
        .into(),
    };

    proto::CredentialLifecycleStateSnapshot { state: Some(state) }
}

fn active_state_to_proto(value: &ActiveCredential) -> proto::ActiveCredentialState {
    proto::ActiveCredentialState {
        metadata: MessageField::some(credential_metadata_to_proto(value.metadata())),
        previous_versions: value.previous_versions().iter().map(credential_ref_to_proto).collect(),
    }
}

fn credential_lifecycle_state_from_proto(
    field: &'static str,
    value: &proto::CredentialLifecycleStateSnapshot,
) -> Result<CredentialLifecycleState, CredentialLifecycleEventPayloadError> {
    let state = value
        .state
        .as_ref()
        .ok_or(CredentialLifecycleEventPayloadError::MissingField { field })?;

    match state {
        CredentialLifecycleStateSnapshotCase::Missing(_) => Ok(CredentialLifecycleState::Missing),
        CredentialLifecycleStateSnapshotCase::PendingWrite(pending) => {
            Ok(CredentialLifecycleState::PendingWrite(PendingCredentialWrite {
                credential_id: decode_credential_id("pending_write.credential_id", &pending.credential_id)?,
                owner_id: decode_owner_id("pending_write.owner_id", &pending.owner_id)?,
                source: decode_source_kind("pending_write.source", pending.source.as_ref())?,
                kind: decode_credential_kind("pending_write.kind", pending.kind.as_ref())?,
            }))
        }
        CredentialLifecycleStateSnapshotCase::Active(active) => Ok(CredentialLifecycleState::Active(
            active_state_from_proto("active", active)?,
        )),
        CredentialLifecycleStateSnapshotCase::WriteFailed(failed) => {
            Ok(CredentialLifecycleState::WriteFailed(FailedCredentialWrite {
                credential_id: decode_credential_id("write_failed.credential_id", &failed.credential_id)?,
                reason: CredentialFailureReason::new(&failed.reason)
                    .map_err(|source| invalid_field("write_failed.reason", source))?,
            }))
        }
        CredentialLifecycleStateSnapshotCase::RotationPending(rotation) => {
            let active = decode_message_field("rotation_pending.active", &rotation.active)?;
            Ok(CredentialLifecycleState::RotationPending(RotationPendingCredential {
                active: active_state_from_proto("rotation_pending.active", active)?,
            }))
        }
        CredentialLifecycleStateSnapshotCase::Revoked(revoked) => {
            let credential_ref = decode_message_field("revoked.credential_ref", &revoked.credential_ref)?;
            Ok(CredentialLifecycleState::Revoked(RevokedCredential {
                credential_ref: decode_credential_ref("revoked.credential_ref", credential_ref)?,
            }))
        }
    }
}

fn active_state_from_proto(
    field: &'static str,
    value: &proto::ActiveCredentialState,
) -> Result<ActiveCredential, CredentialLifecycleEventPayloadError> {
    let metadata = decode_credential_metadata(
        nested_field(field, "metadata"),
        decode_message_field(nested_field(field, "metadata"), &value.metadata)?,
    )?;
    let previous_versions = value
        .previous_versions
        .iter()
        .map(|credential| decode_credential_ref("previous_versions", credential))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ActiveCredential {
        metadata,
        previous_versions,
    })
}

fn credential_metadata_to_proto(value: &CredentialMetadata) -> proto::CredentialMetadata {
    proto::CredentialMetadata {
        reference: MessageField::some(credential_ref_to_proto(value.reference())),
        status: Some(proto_credential_status(value.status()).into()),
        storage_backend: Some(proto_storage_backend(value.storage_backend()).into()),
        fingerprint: value.fingerprint().as_str().to_string(),
    }
}

fn credential_ref_to_proto(value: &CredentialRef) -> proto::CredentialRef {
    proto::CredentialRef {
        id: value.id().as_str().to_string(),
        version: Some(value.version().get()),
        owner_id: value.owner_id().as_str().to_string(),
        source: Some(proto_source_kind(value.source()).into()),
        scope_key: value.scope_key().to_string(),
        kind: Some(proto_credential_kind(value.kind()).into()),
    }
}

fn proto_source_kind(value: SourceKind) -> proto::CredentialSource {
    match value {
        SourceKind::Discord => proto::CredentialSource::CREDENTIAL_SOURCE_DISCORD,
        SourceKind::GitHub => proto::CredentialSource::CREDENTIAL_SOURCE_GITHUB,
        SourceKind::Gitlab => proto::CredentialSource::CREDENTIAL_SOURCE_GITLAB,
        SourceKind::Incidentio => proto::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO,
        SourceKind::Linear => proto::CredentialSource::CREDENTIAL_SOURCE_LINEAR,
        SourceKind::MicrosoftGraph => proto::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH,
        SourceKind::Notion => proto::CredentialSource::CREDENTIAL_SOURCE_NOTION,
        SourceKind::Sentry => proto::CredentialSource::CREDENTIAL_SOURCE_SENTRY,
        SourceKind::Slack => proto::CredentialSource::CREDENTIAL_SOURCE_SLACK,
        SourceKind::Telegram => proto::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM,
        SourceKind::Twitter => proto::CredentialSource::CREDENTIAL_SOURCE_TWITTER,
    }
}

fn proto_credential_kind(value: CredentialKind) -> proto::CredentialKind {
    match value {
        CredentialKind::AppToken => proto::CredentialKind::CREDENTIAL_KIND_APP_TOKEN,
        CredentialKind::BotToken => proto::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN,
        CredentialKind::ClientSecret => proto::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET,
        CredentialKind::ClientState => proto::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE,
        CredentialKind::ConsumerSecret => proto::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET,
        CredentialKind::SigningSecret => proto::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET,
        CredentialKind::SigningToken => proto::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN,
        CredentialKind::VerificationToken => proto::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN,
        CredentialKind::WebhookSecret => proto::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET,
    }
}

fn proto_credential_status(value: CredentialStatus) -> proto::CredentialStatus {
    match value {
        CredentialStatus::Pending => proto::CredentialStatus::CREDENTIAL_STATUS_PENDING,
        CredentialStatus::Active => proto::CredentialStatus::CREDENTIAL_STATUS_ACTIVE,
        CredentialStatus::Previous => proto::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS,
        CredentialStatus::Revoked => proto::CredentialStatus::CREDENTIAL_STATUS_REVOKED,
        CredentialStatus::Expired => proto::CredentialStatus::CREDENTIAL_STATUS_EXPIRED,
    }
}

fn proto_storage_backend(value: StorageBackend) -> proto::StorageBackend {
    match value {
        StorageBackend::InMemory => proto::StorageBackend::STORAGE_BACKEND_IN_MEMORY,
        StorageBackend::OpenBao => proto::StorageBackend::STORAGE_BACKEND_OPENBAO,
        StorageBackend::StaticConfig => proto::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG,
    }
}

fn decode_credential_metadata(
    field: &'static str,
    value: &proto::CredentialMetadata,
) -> Result<CredentialMetadata, CredentialLifecycleEventPayloadError> {
    Ok(CredentialMetadata::new(
        decode_credential_ref(
            nested_field(field, "reference"),
            decode_message_field(nested_field(field, "reference"), &value.reference)?,
        )?,
        decode_credential_status(nested_field(field, "status"), value.status.as_ref())?,
        decode_storage_backend(nested_field(field, "storage_backend"), value.storage_backend.as_ref())?,
        CredentialFingerprint::new(&value.fingerprint)
            .map_err(|source| invalid_field(nested_field(field, "fingerprint"), source))?,
    ))
}

fn decode_credential_ref(
    field: &'static str,
    value: &proto::CredentialRef,
) -> Result<CredentialRef, CredentialLifecycleEventPayloadError> {
    let id = decode_credential_id(nested_field(field, "id"), &value.id)?;
    let owner_id = decode_owner_id(nested_field(field, "owner_id"), &value.owner_id)?;
    let source = decode_source_kind(nested_field(field, "source"), value.source.as_ref())?;
    let kind = decode_credential_kind(nested_field(field, "kind"), value.kind.as_ref())?;
    let version = value
        .version
        .ok_or(CredentialLifecycleEventPayloadError::MissingField {
            field: nested_field(field, "version"),
        })
        .and_then(|value| {
            CredentialVersion::new(value).map_err(|source| invalid_field(nested_field(field, "version"), source))
        })?;
    let scope = decode_scope_key(owner_id, source, nested_field(field, "scope_key"), &value.scope_key)?;
    Ok(CredentialRef::new(id, version, &scope, kind))
}

fn decode_message_field<'a, T: Default>(
    field: &'static str,
    value: &'a MessageField<T>,
) -> Result<&'a T, CredentialLifecycleEventPayloadError> {
    value
        .as_option()
        .ok_or(CredentialLifecycleEventPayloadError::MissingField { field })
}

fn decode_payload<T>(payload: &[u8]) -> Result<T, CredentialLifecycleEventPayloadError>
where
    T: buffa::Message,
{
    T::decode_from_slice(payload).map_err(CredentialLifecycleEventPayloadError::Decode)
}

fn proto_event_type<T>() -> &'static str
where
    T: buffa::MessageName,
{
    T::FULL_NAME
}

fn decode_credential_id(
    field: &'static str,
    value: &str,
) -> Result<CredentialId, CredentialLifecycleEventPayloadError> {
    CredentialId::new(value).map_err(|source| invalid_field(field, source))
}

fn decode_owner_id(
    field: &'static str,
    value: &str,
) -> Result<CredentialOwnerId, CredentialLifecycleEventPayloadError> {
    CredentialOwnerId::new(value).map_err(|source| invalid_field(field, source))
}

fn decode_source_kind(
    field: &'static str,
    value: Option<&EnumValue<proto::CredentialSource>>,
) -> Result<SourceKind, CredentialLifecycleEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto::CredentialSource::CREDENTIAL_SOURCE_DISCORD => Ok(SourceKind::Discord),
        proto::CredentialSource::CREDENTIAL_SOURCE_GITHUB => Ok(SourceKind::GitHub),
        proto::CredentialSource::CREDENTIAL_SOURCE_GITLAB => Ok(SourceKind::Gitlab),
        proto::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO => Ok(SourceKind::Incidentio),
        proto::CredentialSource::CREDENTIAL_SOURCE_LINEAR => Ok(SourceKind::Linear),
        proto::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH => Ok(SourceKind::MicrosoftGraph),
        proto::CredentialSource::CREDENTIAL_SOURCE_NOTION => Ok(SourceKind::Notion),
        proto::CredentialSource::CREDENTIAL_SOURCE_SENTRY => Ok(SourceKind::Sentry),
        proto::CredentialSource::CREDENTIAL_SOURCE_SLACK => Ok(SourceKind::Slack),
        proto::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM => Ok(SourceKind::Telegram),
        proto::CredentialSource::CREDENTIAL_SOURCE_TWITTER => Ok(SourceKind::Twitter),
        proto::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED => Err(invalid_field(field, "unspecified source kind")),
    }
}

fn decode_credential_kind(
    field: &'static str,
    value: Option<&EnumValue<proto::CredentialKind>>,
) -> Result<CredentialKind, CredentialLifecycleEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto::CredentialKind::CREDENTIAL_KIND_APP_TOKEN => Ok(CredentialKind::AppToken),
        proto::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN => Ok(CredentialKind::BotToken),
        proto::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET => Ok(CredentialKind::ClientSecret),
        proto::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE => Ok(CredentialKind::ClientState),
        proto::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET => Ok(CredentialKind::ConsumerSecret),
        proto::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET => Ok(CredentialKind::SigningSecret),
        proto::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN => Ok(CredentialKind::SigningToken),
        proto::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN => Ok(CredentialKind::VerificationToken),
        proto::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET => Ok(CredentialKind::WebhookSecret),
        proto::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED => Err(invalid_field(field, "unspecified credential kind")),
    }
}

fn decode_credential_status(
    field: &'static str,
    value: Option<&EnumValue<proto::CredentialStatus>>,
) -> Result<CredentialStatus, CredentialLifecycleEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto::CredentialStatus::CREDENTIAL_STATUS_PENDING => Ok(CredentialStatus::Pending),
        proto::CredentialStatus::CREDENTIAL_STATUS_ACTIVE => Ok(CredentialStatus::Active),
        proto::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS => Ok(CredentialStatus::Previous),
        proto::CredentialStatus::CREDENTIAL_STATUS_REVOKED => Ok(CredentialStatus::Revoked),
        proto::CredentialStatus::CREDENTIAL_STATUS_EXPIRED => Ok(CredentialStatus::Expired),
        proto::CredentialStatus::CREDENTIAL_STATUS_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified credential status"))
        }
    }
}

fn decode_scope_key(
    owner_id: CredentialOwnerId,
    source: SourceKind,
    field: &'static str,
    scope_key: &str,
) -> Result<CredentialScope, CredentialLifecycleEventPayloadError> {
    if scope_key == source.as_str() {
        return Ok(CredentialScope::source(owner_id, source));
    }

    let Some(integration_id) = scope_key.strip_prefix(&format!("{}/", source.as_str())) else {
        return Err(invalid_field(
            field,
            format!("scope key '{scope_key}' does not match source '{}'", source.as_str()),
        ));
    };
    let integration_id = SourceIntegrationId::new(integration_id).map_err(|source| invalid_field(field, source))?;

    Ok(CredentialScope::integration(owner_id, source, integration_id))
}

fn decode_storage_backend(
    field: &'static str,
    value: Option<&EnumValue<proto::StorageBackend>>,
) -> Result<StorageBackend, CredentialLifecycleEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto::StorageBackend::STORAGE_BACKEND_IN_MEMORY => Ok(StorageBackend::InMemory),
        proto::StorageBackend::STORAGE_BACKEND_OPENBAO => Ok(StorageBackend::OpenBao),
        proto::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG => Ok(StorageBackend::StaticConfig),
        proto::StorageBackend::STORAGE_BACKEND_UNSPECIFIED => Err(invalid_field(field, "unspecified storage backend")),
    }
}

fn decode_known_enum<T>(
    field: &'static str,
    value: Option<&EnumValue<T>>,
) -> Result<T, CredentialLifecycleEventPayloadError>
where
    T: buffa::Enumeration,
{
    let value = value.ok_or(CredentialLifecycleEventPayloadError::MissingField { field })?;
    value
        .as_known()
        .ok_or_else(|| invalid_field(field, format!("unknown enum value {}", value.to_i32())))
}

fn nested_field(parent: &'static str, child: &'static str) -> &'static str {
    match (parent, child) {
        ("metadata", "reference") => "metadata.reference",
        ("metadata", "status") => "metadata.status",
        ("metadata", "storage_backend") => "metadata.storage_backend",
        ("metadata", "fingerprint") => "metadata.fingerprint",
        ("metadata.reference", "id") => "metadata.reference.id",
        ("metadata.reference", "version") => "metadata.reference.version",
        ("metadata.reference", "owner_id") => "metadata.reference.owner_id",
        ("metadata.reference", "source") => "metadata.reference.source",
        ("metadata.reference", "scope_key") => "metadata.reference.scope_key",
        ("metadata.reference", "kind") => "metadata.reference.kind",
        ("credential_ref", "id") => "credential_ref.id",
        ("credential_ref", "version") => "credential_ref.version",
        ("credential_ref", "owner_id") => "credential_ref.owner_id",
        ("credential_ref", "source") => "credential_ref.source",
        ("credential_ref", "scope_key") => "credential_ref.scope_key",
        ("credential_ref", "kind") => "credential_ref.kind",
        ("previous_credential_ref", "id") => "previous_credential_ref.id",
        ("previous_credential_ref", "version") => "previous_credential_ref.version",
        ("previous_credential_ref", "owner_id") => "previous_credential_ref.owner_id",
        ("previous_credential_ref", "source") => "previous_credential_ref.source",
        ("previous_credential_ref", "scope_key") => "previous_credential_ref.scope_key",
        ("previous_credential_ref", "kind") => "previous_credential_ref.kind",
        _ => child,
    }
}

fn invalid_field(field: &'static str, source: impl fmt::Display) -> CredentialLifecycleEventPayloadError {
    CredentialLifecycleEventPayloadError::InvalidField {
        field,
        reason: source.to_string(),
    }
}

impl Decider for RequestCredentialWrite {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    const WRITE_PRECONDITION: Option<WritePrecondition> = Some(WritePrecondition::NoStream);

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_id.as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match state {
            CredentialLifecycleState::Missing => Ok(Decision::event(CredentialLifecycleEvent::WriteRequested {
                credential_id: command.credential_id.clone(),
                owner_id: command.owner_id.clone(),
                source: command.source,
                kind: command.kind,
            })),
            CredentialLifecycleState::Revoked(_) => Err(CredentialLifecycleDecideError::Revoked {
                credential_id: command.credential_id.clone(),
            }),
            _ => Err(CredentialLifecycleDecideError::AlreadyExists {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl Decider for ActivateCredentialWrite {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.metadata.reference().id().as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let pending = match state {
            CredentialLifecycleState::PendingWrite(pending) => pending,
            _ => {
                return Err(CredentialLifecycleDecideError::CredentialWriteNotPending {
                    credential_id: command.metadata.reference().id().clone(),
                });
            }
        };
        validate_activation_metadata(&command.metadata)?;
        validate_ref_matches_pending(command.metadata.reference(), pending)?;

        Ok(Decision::event(CredentialLifecycleEvent::Activated {
            metadata: command.metadata.clone(),
        }))
    }
}

impl Decider for RecordCredentialWriteFailure {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_id.as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        match state {
            CredentialLifecycleState::PendingWrite(_) => Ok(Decision::event(CredentialLifecycleEvent::WriteFailed {
                credential_id: command.credential_id.clone(),
                reason: command.reason.clone(),
            })),
            _ => Err(CredentialLifecycleDecideError::CredentialWriteNotPending {
                credential_id: command.credential_id.clone(),
            }),
        }
    }
}

impl Decider for RequestCredentialRotation {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let active = match state {
            CredentialLifecycleState::Active(active) => active,
            CredentialLifecycleState::RotationPending(_) => {
                return Err(CredentialLifecycleDecideError::CredentialRotationAlreadyPending {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
            _ => {
                return Err(CredentialLifecycleDecideError::CredentialNotActive {
                    credential_id: command.credential_ref.id().clone(),
                });
            }
        };
        validate_same_ref(active.credential_ref(), &command.credential_ref)?;
        Ok(Decision::event(CredentialLifecycleEvent::RotationRequested {
            credential_ref: command.credential_ref.clone(),
        }))
    }
}

impl Decider for RecordCredentialRotationFailure {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let CredentialLifecycleState::RotationPending(rotation) = state else {
            return Err(CredentialLifecycleDecideError::CredentialRotationNotPending {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        validate_same_ref(rotation.active().credential_ref(), &command.credential_ref)?;
        Ok(Decision::event(CredentialLifecycleEvent::RotationFailed {
            credential_ref: command.credential_ref.clone(),
            reason: command.reason.clone(),
        }))
    }
}

impl Decider for ActivateCredentialRotation {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.metadata.reference().id().as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let CredentialLifecycleState::RotationPending(rotation) = state else {
            return Err(CredentialLifecycleDecideError::CredentialRotationNotPending {
                credential_id: command.metadata.reference().id().clone(),
            });
        };
        validate_activation_metadata(&command.metadata)?;
        validate_same_logical_ref(rotation.active.credential_ref(), command.metadata.reference())?;
        validate_newer_version(rotation.active.credential_ref(), command.metadata.reference())?;

        Ok(Decision::event(CredentialLifecycleEvent::Rotated {
            previous_credential_ref: rotation.active.credential_ref().clone(),
            metadata: command.metadata.clone(),
        }))
    }
}

impl Decider for RevokeCredential {
    type StreamId = str;
    type State = CredentialLifecycleState;
    type Event = CredentialLifecycleEvent;
    type DecideError = CredentialLifecycleDecideError;
    type EvolveError = CredentialLifecycleEvolveError;

    fn stream_id(&self) -> &Self::StreamId {
        self.credential_ref.id().as_str()
    }

    fn initial_state() -> Self::State {
        initial_state()
    }

    fn evolve(state: Self::State, event: &Self::Event) -> Result<Self::State, Self::EvolveError> {
        evolve(state, event)
    }

    fn decide(state: &Self::State, command: &Self) -> Result<Decision<Self>, Self::DecideError> {
        let CredentialLifecycleState::Active(active) = state else {
            return Err(CredentialLifecycleDecideError::CredentialNotActive {
                credential_id: command.credential_ref.id().clone(),
            });
        };
        validate_same_ref(active.credential_ref(), &command.credential_ref)?;
        Ok(Decision::event(CredentialLifecycleEvent::Revoked {
            credential_ref: command.credential_ref.clone(),
        }))
    }
}

macro_rules! impl_credential_lifecycle_snapshot_policy {
    ($($command:ty),+ $(,)?) => {
        $(
            impl CommandSnapshotPolicy for $command {
                type SnapshotPolicy = FrequencySnapshot;

                const SNAPSHOT_POLICY: Self::SnapshotPolicy = CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY;
            }
        )+
    };
}

impl_credential_lifecycle_snapshot_policy!(
    RequestCredentialWrite,
    ActivateCredentialWrite,
    RecordCredentialWriteFailure,
    RequestCredentialRotation,
    RecordCredentialRotationFailure,
    ActivateCredentialRotation,
    RevokeCredential,
);

pub fn initial_state() -> CredentialLifecycleState {
    CredentialLifecycleState::Missing
}

pub fn evolve(
    state: CredentialLifecycleState,
    event: &CredentialLifecycleEvent,
) -> Result<CredentialLifecycleState, CredentialLifecycleEvolveError> {
    match event {
        CredentialLifecycleEvent::WriteRequested {
            credential_id,
            owner_id,
            source,
            kind,
        } => match state {
            CredentialLifecycleState::Missing => Ok(CredentialLifecycleState::PendingWrite(PendingCredentialWrite {
                credential_id: credential_id.clone(),
                owner_id: owner_id.clone(),
                source: *source,
                kind: *kind,
            })),
            _ => Err(CredentialLifecycleEvolveError::WriteRequestedAfterStart),
        },
        CredentialLifecycleEvent::WriteFailed { credential_id, reason } => match state {
            CredentialLifecycleState::PendingWrite(pending) if &pending.credential_id == credential_id => {
                Ok(CredentialLifecycleState::WriteFailed(FailedCredentialWrite {
                    credential_id: credential_id.clone(),
                    reason: reason.clone(),
                }))
            }
            CredentialLifecycleState::PendingWrite(pending) => {
                Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
                    expected: pending.credential_id,
                    actual: credential_id.clone(),
                })
            }
            _ => Err(CredentialLifecycleEvolveError::WriteFailedWithoutPendingWrite),
        },
        CredentialLifecycleEvent::Activated { metadata } => match state {
            CredentialLifecycleState::PendingWrite(pending) => {
                validate_activation_metadata_for_evolve(metadata)?;
                validate_ref_matches_pending_for_evolve(metadata.reference(), &pending)?;
                Ok(CredentialLifecycleState::Active(ActiveCredential {
                    metadata: metadata.clone(),
                    previous_versions: Vec::new(),
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::ActivatedWithoutPendingWrite),
        },
        CredentialLifecycleEvent::RotationRequested { credential_ref } => match state {
            CredentialLifecycleState::Active(active) => {
                validate_same_ref_for_evolve(active.credential_ref(), credential_ref)?;
                Ok(CredentialLifecycleState::RotationPending(RotationPendingCredential {
                    active,
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::RotationRequestedWithoutActiveCredential),
        },
        CredentialLifecycleEvent::RotationFailed { credential_ref, .. } => match state {
            CredentialLifecycleState::RotationPending(rotation) => {
                validate_same_ref_for_evolve(rotation.active.credential_ref(), credential_ref)?;
                Ok(CredentialLifecycleState::Active(rotation.active))
            }
            _ => Err(CredentialLifecycleEvolveError::RotationFailedWithoutPendingRotation),
        },
        CredentialLifecycleEvent::Rotated {
            previous_credential_ref,
            metadata,
        } => match state {
            CredentialLifecycleState::RotationPending(rotation) => {
                validate_same_ref_for_evolve(rotation.active.credential_ref(), previous_credential_ref)?;
                validate_activation_metadata_for_evolve(metadata)?;
                validate_same_logical_ref_for_evolve(previous_credential_ref, metadata.reference())?;
                validate_newer_version_for_evolve(previous_credential_ref, metadata.reference())?;
                let mut previous_versions = rotation.active.previous_versions;
                previous_versions.push(previous_credential_ref.clone());
                Ok(CredentialLifecycleState::Active(ActiveCredential {
                    metadata: metadata.clone(),
                    previous_versions,
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::RotatedWithoutPendingRotation),
        },
        CredentialLifecycleEvent::Revoked { credential_ref } => match state {
            CredentialLifecycleState::Active(active) => {
                validate_same_ref_for_evolve(active.credential_ref(), credential_ref)?;
                Ok(CredentialLifecycleState::Revoked(RevokedCredential {
                    credential_ref: credential_ref.clone(),
                }))
            }
            _ => Err(CredentialLifecycleEvolveError::RevokedWithoutActiveCredential),
        },
    }
}

fn validate_activation_metadata(metadata: &CredentialMetadata) -> Result<(), CredentialLifecycleDecideError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialLifecycleDecideError::MetadataNotActive { status });
    }
    Ok(())
}

fn validate_activation_metadata_for_evolve(
    metadata: &CredentialMetadata,
) -> Result<(), CredentialLifecycleEvolveError> {
    let status = metadata.status();
    if status != CredentialStatus::Active {
        return Err(CredentialLifecycleEvolveError::MetadataNotActive { status });
    }
    Ok(())
}

fn validate_ref_matches_pending(
    credential_ref: &CredentialRef,
    pending: &PendingCredentialWrite,
) -> Result<(), CredentialLifecycleDecideError> {
    if credential_ref.id() != &pending.credential_id {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != &pending.owner_id
        || credential_ref.source() != pending.source
        || credential_ref.kind() != pending.kind
    {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

fn validate_ref_matches_pending_for_evolve(
    credential_ref: &CredentialRef,
    pending: &PendingCredentialWrite,
) -> Result<(), CredentialLifecycleEvolveError> {
    if credential_ref.id() != &pending.credential_id {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    if credential_ref.owner_id() != &pending.owner_id
        || credential_ref.source() != pending.source
        || credential_ref.kind() != pending.kind
    {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: pending.credential_id.clone(),
            actual: credential_ref.id().clone(),
        });
    }
    Ok(())
}

fn validate_same_ref(expected: &CredentialRef, actual: &CredentialRef) -> Result<(), CredentialLifecycleDecideError> {
    if expected != actual {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

fn validate_same_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleEvolveError> {
    if expected != actual {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

fn validate_same_logical_ref(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleDecideError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialLifecycleDecideError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

fn validate_same_logical_ref_for_evolve(
    expected: &CredentialRef,
    actual: &CredentialRef,
) -> Result<(), CredentialLifecycleEvolveError> {
    if expected.id() != actual.id()
        || expected.owner_id() != actual.owner_id()
        || expected.source() != actual.source()
        || expected.kind() != actual.kind()
    {
        return Err(CredentialLifecycleEvolveError::CredentialRefMismatch {
            expected: expected.id().clone(),
            actual: actual.id().clone(),
        });
    }
    Ok(())
}

fn validate_newer_version(current: &CredentialRef, next: &CredentialRef) -> Result<(), CredentialLifecycleDecideError> {
    if next.version() <= current.version() {
        return Err(CredentialLifecycleDecideError::RotationVersionNotNewer);
    }
    Ok(())
}

fn validate_newer_version_for_evolve(
    current: &CredentialRef,
    next: &CredentialRef,
) -> Result<(), CredentialLifecycleEvolveError> {
    if next.version() <= current.version() {
        return Err(CredentialLifecycleEvolveError::RotationVersionNotNewer);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use chrono::Utc;
    use trogon_decider::testing::TestCase;
    use trogon_decider_runtime::{
        AppendStreamRequest, AppendStreamResponse, CommandError, CommandExecution, EventData, EventDecode,
        EventDecodeOutcome, EventEncode, EventType, ReadFrom, ReadStreamRequest, ReadStreamResponse, StreamAppend,
        StreamEvent, StreamPosition, StreamRead, StreamWritePrecondition,
    };

    use super::*;
    use crate::secret_store::{CredentialFingerprint, CredentialScope, CredentialVersion, StorageBackend};

    #[derive(Debug, thiserror::Error)]
    #[error("lifecycle test stream store rejected the append")]
    struct LifecycleTestStoreError;

    #[derive(Default)]
    struct LifecycleTestStore {
        events: Mutex<Vec<StreamEvent>>,
        write_preconditions: Mutex<Vec<StreamWritePrecondition>>,
    }

    impl LifecycleTestStore {
        fn write_preconditions(&self) -> Vec<StreamWritePrecondition> {
            self.write_preconditions.lock().unwrap().clone()
        }

        fn events(&self) -> Vec<StreamEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    impl StreamRead<str> for LifecycleTestStore {
        type Error = LifecycleTestStoreError;

        async fn read_stream(&self, request: ReadStreamRequest<'_, str>) -> Result<ReadStreamResponse, Self::Error> {
            let start = match request.from {
                ReadFrom::Beginning => 1,
                ReadFrom::Position(position) => position.as_u64(),
            };
            let events = self.events.lock().unwrap();
            let stream_events = events
                .iter()
                .filter(|event| event.stream_id() == request.stream_id && event.stream_position.as_u64() >= start)
                .cloned()
                .collect();
            Ok(ReadStreamResponse {
                current_position: current_position(&events, request.stream_id),
                events: stream_events,
            })
        }
    }

    impl StreamAppend<str> for LifecycleTestStore {
        type Error = LifecycleTestStoreError;

        async fn append_stream(
            &self,
            request: AppendStreamRequest<'_, str>,
        ) -> Result<AppendStreamResponse, Self::Error> {
            let mut events = self.events.lock().unwrap();
            let current_position = current_position(&events, request.stream_id);
            self.write_preconditions
                .lock()
                .unwrap()
                .push(request.stream_write_precondition);
            match request.stream_write_precondition {
                StreamWritePrecondition::Any => {}
                StreamWritePrecondition::StreamExists if current_position.is_some() => {}
                StreamWritePrecondition::NoStream if current_position.is_none() => {}
                StreamWritePrecondition::At(position) if current_position == Some(position) => {}
                _ => return Err(LifecycleTestStoreError),
            }

            let mut last_position = current_position;
            for event in request.events {
                let stream_position = position(events.len() as u64 + 1);
                last_position = Some(stream_position);
                events.push(StreamEvent {
                    stream_id: request.stream_id.to_string(),
                    event,
                    stream_position,
                    recorded_at: Utc::now(),
                });
            }

            Ok(AppendStreamResponse {
                stream_position: last_position.expect("append request must contain events"),
            })
        }
    }

    fn current_position(events: &[StreamEvent], stream_id: &str) -> Option<StreamPosition> {
        events
            .iter()
            .filter(|event| event.stream_id() == stream_id)
            .map(|event| event.stream_position)
            .max()
    }

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).unwrap()
    }

    fn credential_id() -> CredentialId {
        CredentialId::new("openbao:tenant-1:github/primary:webhook_secret").unwrap()
    }

    fn owner_id() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
    }

    fn credential_ref(version: u64) -> CredentialRef {
        let scope = CredentialScope::integration(
            owner_id(),
            SourceKind::GitHub,
            crate::source_integration_id::SourceIntegrationId::new("primary").unwrap(),
        );
        CredentialRef::new(
            credential_id(),
            CredentialVersion::new(version).unwrap(),
            &scope,
            CredentialKind::WebhookSecret,
        )
    }

    fn metadata(version: u64) -> CredentialMetadata {
        let credential_ref = credential_ref(version);
        CredentialMetadata::new(
            credential_ref.clone(),
            CredentialStatus::Active,
            StorageBackend::OpenBao,
            CredentialFingerprint::new(format!("openbao:secret/metadata/{credential_ref}")).unwrap(),
        )
    }

    fn write_requested() -> CredentialLifecycleEvent {
        CredentialLifecycleEvent::WriteRequested {
            credential_id: credential_id(),
            owner_id: owner_id(),
            source: SourceKind::GitHub,
            kind: CredentialKind::WebhookSecret,
        }
    }

    fn activated(version: u64) -> CredentialLifecycleEvent {
        CredentialLifecycleEvent::Activated {
            metadata: metadata(version),
        }
    }

    fn rotation_requested(version: u64) -> CredentialLifecycleEvent {
        CredentialLifecycleEvent::RotationRequested {
            credential_ref: credential_ref(version),
        }
    }

    fn rotation_failed(version: u64) -> CredentialLifecycleEvent {
        CredentialLifecycleEvent::RotationFailed {
            credential_ref: credential_ref(version),
            reason: CredentialFailureReason::new("openbao rotate failed").unwrap(),
        }
    }

    fn rotated(previous_version: u64, next_version: u64) -> CredentialLifecycleEvent {
        CredentialLifecycleEvent::Rotated {
            previous_credential_ref: credential_ref(previous_version),
            metadata: metadata(next_version),
        }
    }

    fn revoked(version: u64) -> CredentialLifecycleEvent {
        CredentialLifecycleEvent::Revoked {
            credential_ref: credential_ref(version),
        }
    }

    fn request_write() -> RequestCredentialWrite {
        RequestCredentialWrite::new(
            credential_id(),
            owner_id(),
            SourceKind::GitHub,
            CredentialKind::WebhookSecret,
        )
    }

    fn lifecycle_events_rebuild(
        events: impl IntoIterator<Item = CredentialLifecycleEvent>,
    ) -> CredentialLifecycleState {
        events
            .into_iter()
            .try_fold(initial_state(), |state, event| evolve(state, &event))
            .unwrap()
    }

    #[test]
    fn snapshot_policy_uses_scheduler_frequency_pattern() {
        assert_eq!(CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY.frequency().get(), 32);
        assert_eq!(
            <RequestCredentialWrite as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
        assert_eq!(
            <ActivateCredentialWrite as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
        assert_eq!(
            <RecordCredentialWriteFailure as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
        assert_eq!(
            <RequestCredentialRotation as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
        assert_eq!(
            <RecordCredentialRotationFailure as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
        assert_eq!(
            <ActivateCredentialRotation as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
        assert_eq!(
            <RevokeCredential as CommandSnapshotPolicy>::SNAPSHOT_POLICY,
            CREDENTIAL_LIFECYCLE_SNAPSHOT_POLICY
        );
    }

    #[test]
    fn lifecycle_state_snapshot_round_trips_active_state() {
        let state = lifecycle_events_rebuild([write_requested(), activated(1), rotation_requested(1), rotated(1, 2)]);

        let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
        let decoded =
            <CredentialLifecycleState as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded)).unwrap();

        assert_eq!(decoded, state);
    }

    #[test]
    fn lifecycle_state_snapshot_round_trips_pending_write_state() {
        let state = lifecycle_events_rebuild([write_requested()]);

        let encoded = SnapshotPayloadEncode::encode(&state).unwrap();
        let decoded =
            <CredentialLifecycleState as SnapshotPayloadDecode>::decode(SnapshotPayloadData::new(&encoded)).unwrap();

        assert_eq!(decoded, state);
    }

    #[test]
    fn given_when_then_requests_credential_write() {
        TestCase::<RequestCredentialWrite>::new()
            .given_no_history()
            .when(request_write())
            .then([write_requested()]);
    }

    #[test]
    fn given_when_then_rejects_duplicate_credential_write() {
        TestCase::<RequestCredentialWrite>::new()
            .given([write_requested()])
            .when(request_write())
            .then_error(CredentialLifecycleDecideError::AlreadyExists {
                credential_id: credential_id(),
            });
    }

    #[test]
    fn given_when_then_activates_pending_credential_write() {
        TestCase::<ActivateCredentialWrite>::new()
            .given([write_requested()])
            .when(ActivateCredentialWrite::new(metadata(1)))
            .then([activated(1)]);
    }

    #[test]
    fn given_when_then_records_pending_write_failure() {
        let reason = CredentialFailureReason::new("openbao write failed").unwrap();

        TestCase::<RecordCredentialWriteFailure>::new()
            .given([write_requested()])
            .when(RecordCredentialWriteFailure::new(credential_id(), reason.clone()))
            .then([CredentialLifecycleEvent::WriteFailed {
                credential_id: credential_id(),
                reason,
            }]);
    }

    #[test]
    fn given_when_then_requests_rotation_for_active_credential() {
        TestCase::<RequestCredentialRotation>::new()
            .given([write_requested()])
            .given([activated(1)])
            .when(RequestCredentialRotation::new(credential_ref(1)))
            .then([rotation_requested(1)]);
    }

    #[test]
    fn given_when_then_activates_pending_rotation() {
        TestCase::<ActivateCredentialRotation>::new()
            .given([write_requested()])
            .given([activated(1)])
            .given([rotation_requested(1)])
            .when(ActivateCredentialRotation::new(metadata(2)))
            .then([rotated(1, 2)]);
    }

    #[test]
    fn given_when_then_records_pending_rotation_failure() {
        let reason = CredentialFailureReason::new("openbao rotate failed").unwrap();

        TestCase::<RecordCredentialRotationFailure>::new()
            .given([write_requested()])
            .given([activated(1)])
            .given([rotation_requested(1)])
            .when(RecordCredentialRotationFailure::new(credential_ref(1), reason.clone()))
            .then([CredentialLifecycleEvent::RotationFailed {
                credential_ref: credential_ref(1),
                reason,
            }]);
    }

    #[test]
    fn given_when_then_rejects_duplicate_rotation_request() {
        TestCase::<RequestCredentialRotation>::new()
            .given([write_requested()])
            .given([activated(1)])
            .given([rotation_requested(1)])
            .when(RequestCredentialRotation::new(credential_ref(1)))
            .then_error(CredentialLifecycleDecideError::CredentialRotationAlreadyPending {
                credential_id: credential_id(),
            });
    }

    #[test]
    fn given_when_then_rejects_rotation_with_stale_version() {
        TestCase::<ActivateCredentialRotation>::new()
            .given([write_requested()])
            .given([activated(1)])
            .given([rotation_requested(1)])
            .when(ActivateCredentialRotation::new(metadata(1)))
            .then_error(CredentialLifecycleDecideError::RotationVersionNotNewer);
    }

    #[test]
    fn given_when_then_allows_rotation_retry_after_rotation_failure() {
        TestCase::<RequestCredentialRotation>::new()
            .given([write_requested()])
            .given([activated(1)])
            .given([rotation_requested(1)])
            .given([rotation_failed(1)])
            .when(RequestCredentialRotation::new(credential_ref(1)))
            .then([rotation_requested(1)]);
    }

    #[test]
    fn given_when_then_revokes_active_credential() {
        TestCase::<RevokeCredential>::new()
            .given([write_requested()])
            .given([activated(1)])
            .when(RevokeCredential::new(credential_ref(1)))
            .then([revoked(1)]);
    }

    #[test]
    fn lifecycle_events_rebuild_active_rotated_state() {
        let state = [write_requested(), activated(1), rotation_requested(1), rotated(1, 2)]
            .into_iter()
            .try_fold(initial_state(), |state, event| evolve(state, &event))
            .unwrap();

        let CredentialLifecycleState::Active(active) = state else {
            panic!("expected active credential");
        };
        assert_eq!(active.credential_ref(), &credential_ref(2));
        assert_eq!(active.previous_versions(), &[credential_ref(1)]);
    }

    #[test]
    fn lifecycle_event_codec_round_trips_all_events() {
        for event in [
            write_requested(),
            CredentialLifecycleEvent::WriteFailed {
                credential_id: credential_id(),
                reason: CredentialFailureReason::new("openbao unavailable").unwrap(),
            },
            activated(1),
            rotation_requested(1),
            rotation_failed(1),
            rotated(1, 2),
            revoked(2),
        ] {
            let event_type = event.event_type().unwrap();
            let payload = event.encode().unwrap();
            let decoded = CredentialLifecycleEvent::decode(EventData::new(event_type, &payload))
                .unwrap()
                .into_decoded();

            assert_eq!(decoded, Some(event));
        }
    }

    #[test]
    fn lifecycle_event_codec_preserves_integration_scope_key() {
        let event = activated(1);
        let payload = event.encode().unwrap();
        let proto_event = proto::CredentialActivated::decode_from_slice(&payload).unwrap();
        let metadata = proto_event.metadata.as_option().unwrap();
        let reference = metadata.reference.as_option().unwrap();

        assert_eq!(reference.scope_key, "github/primary");

        let decoded = CredentialLifecycleEvent::decode(EventData::new(event.event_type().unwrap(), &payload))
            .unwrap()
            .into_decoded()
            .unwrap();
        let CredentialLifecycleEvent::Activated { metadata } = decoded else {
            panic!("expected activated event");
        };
        assert_eq!(metadata.reference().scope_key(), "github/primary");
    }

    #[test]
    fn lifecycle_event_codec_skips_foreign_event_types() {
        let decoded = CredentialLifecycleEvent::decode(EventData::new("foreign.event.v1", b"{}")).unwrap();

        assert_eq!(decoded, EventDecodeOutcome::Skipped);
    }

    #[test]
    fn lifecycle_event_codec_rejects_invalid_persisted_fields() {
        let payload = proto::CredentialWriteRequested {
            credential_id: String::new(),
            owner_id: "tenant-1".to_string(),
            source: Some(proto::CredentialSource::CREDENTIAL_SOURCE_GITHUB.into()),
            kind: Some(proto::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET.into()),
        }
        .encode_to_vec();

        let error = CredentialLifecycleEvent::decode(EventData::new(
            proto_event_type::<proto::CredentialWriteRequested>(),
            &payload,
        ))
        .unwrap_err();

        assert!(matches!(
            error,
            CredentialLifecycleEventPayloadError::InvalidField {
                field: "credential_id",
                ..
            }
        ));
    }

    #[test]
    fn lifecycle_event_codec_rejects_scope_key_that_does_not_match_source() {
        let payload = proto::CredentialRotationRequested {
            credential_ref: MessageField::some(proto::CredentialRef {
                id: "openbao:tenant-1:github/primary:webhook_secret".to_string(),
                version: Some(1),
                owner_id: "tenant-1".to_string(),
                source: Some(proto::CredentialSource::CREDENTIAL_SOURCE_GITHUB.into()),
                scope_key: "slack/primary".to_string(),
                kind: Some(proto::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET.into()),
            }),
        }
        .encode_to_vec();

        let error = CredentialLifecycleEvent::decode(EventData::new(
            proto_event_type::<proto::CredentialRotationRequested>(),
            &payload,
        ))
        .unwrap_err();

        assert!(matches!(
            error,
            CredentialLifecycleEventPayloadError::InvalidField {
                field: "credential_ref.scope_key",
                ..
            }
        ));
    }

    #[tokio::test]
    async fn command_execution_persists_and_replays_lifecycle_events() {
        let store = LifecycleTestStore::default();

        let request_result = CommandExecution::new(&store, &request_write()).execute().await.unwrap();
        assert_eq!(request_result.stream_position, position(1));
        assert_eq!(request_result.events.as_slice(), &[write_requested()]);
        assert!(matches!(
            request_result.state,
            CredentialLifecycleState::PendingWrite(_)
        ));
        assert_eq!(store.write_preconditions(), [StreamWritePrecondition::NoStream]);

        let activation = ActivateCredentialWrite::new(metadata(1));
        let activation_result = CommandExecution::new(&store, &activation).execute().await.unwrap();
        assert_eq!(activation_result.stream_position, position(2));
        assert_eq!(activation_result.events.as_slice(), &[activated(1)]);
        assert_eq!(
            store.write_preconditions(),
            [
                StreamWritePrecondition::NoStream,
                StreamWritePrecondition::At(position(1))
            ]
        );

        let CredentialLifecycleState::Active(active) = activation_result.state else {
            panic!("expected active credential");
        };
        assert_eq!(active.credential_ref(), &credential_ref(1));
    }

    #[tokio::test]
    async fn command_execution_rejects_duplicate_write_with_no_stream_precondition() {
        let store = LifecycleTestStore::default();

        CommandExecution::new(&store, &request_write()).execute().await.unwrap();
        let error = CommandExecution::new(&store, &request_write())
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(error, CommandError::Append(LifecycleTestStoreError)));
        assert_eq!(
            store.write_preconditions(),
            [StreamWritePrecondition::NoStream, StreamWritePrecondition::NoStream]
        );
        assert_eq!(store.events().len(), 1);
    }
}
