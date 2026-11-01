use std::borrow::Cow;
use std::fmt;

use buffa::{EnumValue, MessageField, ProtoBox};
use trogonai_proto::gateway::credentials::{state_v1, v1};

use super::domain::{
    CredentialDestroyReason, CredentialDestroyReasonError, CredentialFailureReason, CredentialFailureReasonError,
    CredentialFingerprint, CredentialFingerprintError, CredentialId, CredentialIdError, CredentialKind,
    CredentialMetadata, CredentialOwnerId, CredentialOwnerIdError, CredentialRef, CredentialScope, CredentialStatus,
    CredentialVersion, CredentialVersionError, SourceKind, StorageBackend, StorageBackendError,
};
use crate::source_integration_id::{SourceIntegrationId, SourceIntegrationIdError};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CredentialProtoDecodeError {
    #[error("credential field '{field}' is missing")]
    MissingField { field: CredentialFieldPath },
    #[error("credential field '{field}' is invalid: {reason}")]
    InvalidField {
        field: CredentialFieldPath,
        #[source]
        reason: CredentialFieldInvalidError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CredentialFieldInvalidError {
    #[error(transparent)]
    CredentialId(#[from] CredentialIdError),
    #[error(transparent)]
    OwnerId(#[from] CredentialOwnerIdError),
    #[error(transparent)]
    Fingerprint(#[from] CredentialFingerprintError),
    #[error(transparent)]
    StorageBackend(#[from] StorageBackendError),
    #[error(transparent)]
    FailureReason(#[from] CredentialFailureReasonError),
    #[error(transparent)]
    DestroyReason(#[from] CredentialDestroyReasonError),
    #[error(transparent)]
    Version(#[from] CredentialVersionError),
    #[error(transparent)]
    SourceIntegrationId(#[from] SourceIntegrationIdError),
    #[error("credential scope key must not be empty")]
    EmptyScopeKey,
    #[error("scope key '{scope_key}' does not match source '{source_kind}'")]
    ScopeKeyMismatch {
        scope_key: String,
        source_kind: &'static str,
    },
    #[error("unspecified {0}")]
    UnspecifiedEnum(UnspecifiedEnumKind),
    #[error("unknown enum value {0}")]
    UnknownEnumValue(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UnspecifiedEnumKind {
    SourceKind,
    CredentialKind,
    CredentialStatus,
    StorageBackend,
}

impl fmt::Display for UnspecifiedEnumKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SourceKind => "source kind",
            Self::CredentialKind => "credential kind",
            Self::CredentialStatus => "credential status",
            Self::StorageBackend => "storage backend",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CredentialFieldPath(Cow<'static, str>);

impl CredentialFieldPath {
    pub(crate) fn child(&self, name: &'static str) -> Self {
        Self(Cow::Owned(format!("{}.{name}", self.0)))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for CredentialFieldPath {
    fn from(value: &'static str) -> Self {
        Self(Cow::Borrowed(value))
    }
}

impl From<&CredentialFieldPath> for CredentialFieldPath {
    fn from(value: &CredentialFieldPath) -> Self {
        value.clone()
    }
}

impl PartialEq<&str> for CredentialFieldPath {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl fmt::Display for CredentialFieldPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) fn credential_ref_to_proto(value: &CredentialRef) -> v1::CredentialRef {
    v1::CredentialRef {
        id: value.id().as_str().to_string(),
        version: Some(value.version().get()),
        owner_id: value.owner_id().as_str().to_string(),
        source: Some(proto_source_kind(value.source()).into()),
        scope_key: value.scope_key().to_string(),
        kind: Some(proto_credential_kind(value.kind()).into()),
    }
}

pub(crate) fn credential_metadata_to_proto(value: &CredentialMetadata) -> v1::CredentialMetadata {
    v1::CredentialMetadata {
        reference: MessageField::some(credential_ref_to_proto(value.reference())),
        status: Some(proto_credential_status(value.status()).into()),
        fingerprint: value.fingerprint().as_str().to_string(),
    }
}

pub(crate) fn decode_credential_ref(
    field: impl Into<CredentialFieldPath>,
    value: &v1::CredentialRef,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let id = decode_credential_id(field.child("id"), &value.id)?;
    let owner_id = decode_owner_id(field.child("owner_id"), &value.owner_id)?;
    let source = decode_source_kind(field.child("source"), value.source.as_ref())?;
    let kind = decode_credential_kind(field.child("kind"), value.kind.as_ref())?;
    let version = value
        .version
        .ok_or(CredentialProtoDecodeError::MissingField {
            field: field.child("version"),
        })
        .and_then(|value| {
            CredentialVersion::new(value).map_err(|source| invalid_field(field.child("version"), source))
        })?;
    let scope = decode_scope_key(owner_id, source, field.child("scope_key"), &value.scope_key)?;
    Ok(CredentialRef::new(id, version, &scope, kind))
}

pub(crate) fn write_requested_to_proto(
    credential_id: &CredentialId,
    scope: &CredentialScope,
    kind: CredentialKind,
) -> v1::CredentialWriteRequested {
    v1::CredentialWriteRequested {
        credential_id: credential_id.as_str().to_string(),
        owner_id: scope.owner_id().as_str().to_string(),
        source: Some(proto_source_kind(scope.source_kind()).into()),
        kind: Some(proto_credential_kind(kind).into()),
        scope_key: scope.scope_key(),
    }
}

pub(crate) fn write_failed_to_proto(
    credential_id: &CredentialId,
    reason: &CredentialFailureReason,
) -> v1::CredentialWriteFailed {
    v1::CredentialWriteFailed {
        credential_id: credential_id.as_str().to_string(),
        reason: reason.as_str().to_string(),
    }
}

pub(crate) fn activated_to_proto(metadata: &CredentialMetadata) -> v1::CredentialActivated {
    v1::CredentialActivated {
        metadata: MessageField::some(credential_metadata_to_proto(metadata)),
    }
}

pub(crate) fn rotation_requested_to_proto(credential_ref: &CredentialRef) -> v1::CredentialRotationRequested {
    v1::CredentialRotationRequested {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
    }
}

pub(crate) fn rotation_failed_to_proto(
    credential_ref: &CredentialRef,
    reason: &CredentialFailureReason,
) -> v1::CredentialRotationFailed {
    v1::CredentialRotationFailed {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
        reason: reason.as_str().to_string(),
    }
}

pub(crate) fn rotated_to_proto(
    previous_credential_ref: &CredentialRef,
    metadata: &CredentialMetadata,
) -> v1::CredentialRotated {
    v1::CredentialRotated {
        previous_credential_ref: MessageField::some(credential_ref_to_proto(previous_credential_ref)),
        metadata: MessageField::some(credential_metadata_to_proto(metadata)),
    }
}

pub(crate) fn revoked_to_proto(credential_ref: &CredentialRef) -> v1::CredentialRevoked {
    v1::CredentialRevoked {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
    }
}

pub(crate) fn destroy_requested_to_proto(
    credential_ref: &CredentialRef,
    reason: &CredentialDestroyReason,
) -> v1::CredentialDestroyRequested {
    v1::CredentialDestroyRequested {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
        reason: reason.as_str().to_string(),
    }
}

pub(crate) fn destroyed_to_proto(credential_ref: &CredentialRef) -> v1::CredentialDestroyed {
    v1::CredentialDestroyed {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
    }
}

pub(crate) fn destroy_failed_to_proto(
    credential_ref: &CredentialRef,
    reason: &CredentialFailureReason,
) -> v1::CredentialDestroyFailed {
    v1::CredentialDestroyFailed {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
        reason: reason.as_str().to_string(),
    }
}

pub(crate) fn decode_write_requested(
    event: &v1::CredentialWriteRequested,
) -> Result<(CredentialId, CredentialScope, CredentialKind), CredentialProtoDecodeError> {
    let credential_id = decode_credential_id("credential_id", &event.credential_id)?;
    let owner_id = decode_owner_id("owner_id", &event.owner_id)?;
    let source = decode_source_kind("source", event.source.as_ref())?;
    let kind = decode_credential_kind("kind", event.kind.as_ref())?;
    let scope = decode_scope_key(owner_id, source, "scope_key", &event.scope_key)?;
    Ok((credential_id, scope, kind))
}

pub(crate) fn decode_credential_metadata(
    field: impl Into<CredentialFieldPath>,
    value: &v1::CredentialMetadata,
) -> Result<CredentialMetadata, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let reference_field = field.child("reference");
    let reference = decode_credential_ref(
        &reference_field,
        decode_message_field(&reference_field, &value.reference)?,
    )?;
    let status = decode_credential_status(field.child("status"), value.status.as_ref())?;
    let storage_backend = StorageBackend::from_credential_id(reference.id())
        .map_err(|source| invalid_field(reference_field.child("id"), source))?;
    let fingerprint = CredentialFingerprint::new(&value.fingerprint)
        .map_err(|source| invalid_field(field.child("fingerprint"), source))?;
    Ok(CredentialMetadata::new(reference, status, storage_backend, fingerprint))
}

pub(crate) fn decode_write_failed(
    event: &v1::CredentialWriteFailed,
) -> Result<(CredentialId, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_id("credential_id", &event.credential_id)?,
        CredentialFailureReason::new(&event.reason).map_err(|source| invalid_field("reason", source))?,
    ))
}

pub(crate) fn decode_rotation_requested(
    event: &v1::CredentialRotationRequested,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_credential_ref(
        "credential_ref",
        decode_message_field("credential_ref", &event.credential_ref)?,
    )
}

pub(crate) fn decode_rotation_failed(
    event: &v1::CredentialRotationFailed,
) -> Result<(CredentialRef, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_ref(
            "credential_ref",
            decode_message_field("credential_ref", &event.credential_ref)?,
        )?,
        CredentialFailureReason::new(&event.reason).map_err(|source| invalid_field("reason", source))?,
    ))
}

pub(crate) fn decode_rotated(
    event: &v1::CredentialRotated,
) -> Result<(CredentialRef, CredentialMetadata), CredentialProtoDecodeError> {
    Ok((
        decode_credential_ref(
            "previous_credential_ref",
            decode_message_field("previous_credential_ref", &event.previous_credential_ref)?,
        )?,
        decode_credential_metadata("metadata", decode_message_field("metadata", &event.metadata)?)?,
    ))
}

pub(crate) fn decode_revoked(event: &v1::CredentialRevoked) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_credential_ref(
        "credential_ref",
        decode_message_field("credential_ref", &event.credential_ref)?,
    )
}

pub(crate) fn decode_destroy_requested(
    event: &v1::CredentialDestroyRequested,
) -> Result<(CredentialRef, CredentialDestroyReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_ref(
            "credential_ref",
            decode_message_field("credential_ref", &event.credential_ref)?,
        )?,
        CredentialDestroyReason::new(&event.reason).map_err(|source| invalid_field("reason", source))?,
    ))
}

pub(crate) fn decode_destroyed(event: &v1::CredentialDestroyed) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_credential_ref(
        "credential_ref",
        decode_message_field("credential_ref", &event.credential_ref)?,
    )
}

pub(crate) fn decode_destroy_failed(
    event: &v1::CredentialDestroyFailed,
) -> Result<(CredentialRef, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_ref(
            "credential_ref",
            decode_message_field("credential_ref", &event.credential_ref)?,
        )?,
        CredentialFailureReason::new(&event.reason).map_err(|source| invalid_field("reason", source))?,
    ))
}

fn proto_source_kind(value: SourceKind) -> v1::CredentialSource {
    match value {
        SourceKind::Discord => v1::CredentialSource::CREDENTIAL_SOURCE_DISCORD,
        SourceKind::GitHub => v1::CredentialSource::CREDENTIAL_SOURCE_GITHUB,
        SourceKind::Gitlab => v1::CredentialSource::CREDENTIAL_SOURCE_GITLAB,
        SourceKind::Incidentio => v1::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO,
        SourceKind::Linear => v1::CredentialSource::CREDENTIAL_SOURCE_LINEAR,
        SourceKind::MicrosoftGraph => v1::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH,
        SourceKind::Notion => v1::CredentialSource::CREDENTIAL_SOURCE_NOTION,
        SourceKind::Sentry => v1::CredentialSource::CREDENTIAL_SOURCE_SENTRY,
        SourceKind::Slack => v1::CredentialSource::CREDENTIAL_SOURCE_SLACK,
        SourceKind::Telegram => v1::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM,
        SourceKind::Twitter => v1::CredentialSource::CREDENTIAL_SOURCE_TWITTER,
        SourceKind::Datadog => v1::CredentialSource::CREDENTIAL_SOURCE_DATADOG,
        SourceKind::GooglePubSub => v1::CredentialSource::CREDENTIAL_SOURCE_GOOGLE_PUBSUB,
        SourceKind::Zoom => v1::CredentialSource::CREDENTIAL_SOURCE_ZOOM,
        SourceKind::Plaid => v1::CredentialSource::CREDENTIAL_SOURCE_PLAID,
    }
}

fn proto_credential_kind(value: CredentialKind) -> v1::CredentialKind {
    match value {
        CredentialKind::AppToken => v1::CredentialKind::CREDENTIAL_KIND_APP_TOKEN,
        CredentialKind::BotToken => v1::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN,
        CredentialKind::ClientSecret => v1::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET,
        CredentialKind::ClientState => v1::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE,
        CredentialKind::ConsumerSecret => v1::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET,
        CredentialKind::SigningSecret => v1::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET,
        CredentialKind::SigningToken => v1::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN,
        CredentialKind::VerificationToken => v1::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN,
        CredentialKind::WebhookSecret => v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET,
        CredentialKind::WebhookToken => v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_TOKEN,
        CredentialKind::SecretToken => v1::CredentialKind::CREDENTIAL_KIND_SECRET_TOKEN,
        CredentialKind::ServiceAccountCredentials => v1::CredentialKind::CREDENTIAL_KIND_SERVICE_ACCOUNT_CREDENTIALS,
    }
}

fn proto_credential_status(value: CredentialStatus) -> v1::CredentialStatus {
    match value {
        CredentialStatus::Pending => v1::CredentialStatus::CREDENTIAL_STATUS_PENDING,
        CredentialStatus::Active => v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE,
        CredentialStatus::Previous => v1::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS,
        CredentialStatus::Revoked => v1::CredentialStatus::CREDENTIAL_STATUS_REVOKED,
        CredentialStatus::Expired => v1::CredentialStatus::CREDENTIAL_STATUS_EXPIRED,
        CredentialStatus::Destroyed => v1::CredentialStatus::CREDENTIAL_STATUS_DESTROYED,
    }
}

fn decode_source_kind(
    field: impl Into<CredentialFieldPath>,
    value: Option<&EnumValue<v1::CredentialSource>>,
) -> Result<SourceKind, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    match decode_known_enum(&field, value)? {
        v1::CredentialSource::CREDENTIAL_SOURCE_DISCORD => Ok(SourceKind::Discord),
        v1::CredentialSource::CREDENTIAL_SOURCE_GITHUB => Ok(SourceKind::GitHub),
        v1::CredentialSource::CREDENTIAL_SOURCE_GITLAB => Ok(SourceKind::Gitlab),
        v1::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO => Ok(SourceKind::Incidentio),
        v1::CredentialSource::CREDENTIAL_SOURCE_LINEAR => Ok(SourceKind::Linear),
        v1::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH => Ok(SourceKind::MicrosoftGraph),
        v1::CredentialSource::CREDENTIAL_SOURCE_NOTION => Ok(SourceKind::Notion),
        v1::CredentialSource::CREDENTIAL_SOURCE_SENTRY => Ok(SourceKind::Sentry),
        v1::CredentialSource::CREDENTIAL_SOURCE_SLACK => Ok(SourceKind::Slack),
        v1::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM => Ok(SourceKind::Telegram),
        v1::CredentialSource::CREDENTIAL_SOURCE_TWITTER => Ok(SourceKind::Twitter),
        v1::CredentialSource::CREDENTIAL_SOURCE_DATADOG => Ok(SourceKind::Datadog),
        v1::CredentialSource::CREDENTIAL_SOURCE_GOOGLE_PUBSUB => Ok(SourceKind::GooglePubSub),
        v1::CredentialSource::CREDENTIAL_SOURCE_ZOOM => Ok(SourceKind::Zoom),
        v1::CredentialSource::CREDENTIAL_SOURCE_PLAID => Ok(SourceKind::Plaid),
        v1::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED => Err(invalid_field(
            field,
            CredentialFieldInvalidError::UnspecifiedEnum(UnspecifiedEnumKind::SourceKind),
        )),
    }
}

fn decode_credential_kind(
    field: impl Into<CredentialFieldPath>,
    value: Option<&EnumValue<v1::CredentialKind>>,
) -> Result<CredentialKind, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    match decode_known_enum(&field, value)? {
        v1::CredentialKind::CREDENTIAL_KIND_APP_TOKEN => Ok(CredentialKind::AppToken),
        v1::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN => Ok(CredentialKind::BotToken),
        v1::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET => Ok(CredentialKind::ClientSecret),
        v1::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE => Ok(CredentialKind::ClientState),
        v1::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET => Ok(CredentialKind::ConsumerSecret),
        v1::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET => Ok(CredentialKind::SigningSecret),
        v1::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN => Ok(CredentialKind::SigningToken),
        v1::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN => Ok(CredentialKind::VerificationToken),
        v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET => Ok(CredentialKind::WebhookSecret),
        v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_TOKEN => Ok(CredentialKind::WebhookToken),
        v1::CredentialKind::CREDENTIAL_KIND_SECRET_TOKEN => Ok(CredentialKind::SecretToken),
        v1::CredentialKind::CREDENTIAL_KIND_SERVICE_ACCOUNT_CREDENTIALS => {
            Ok(CredentialKind::ServiceAccountCredentials)
        }
        v1::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED => Err(invalid_field(
            field,
            CredentialFieldInvalidError::UnspecifiedEnum(UnspecifiedEnumKind::CredentialKind),
        )),
    }
}

fn decode_credential_status(
    field: impl Into<CredentialFieldPath>,
    value: Option<&EnumValue<v1::CredentialStatus>>,
) -> Result<CredentialStatus, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    match decode_known_enum(&field, value)? {
        v1::CredentialStatus::CREDENTIAL_STATUS_PENDING => Ok(CredentialStatus::Pending),
        v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE => Ok(CredentialStatus::Active),
        v1::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS => Ok(CredentialStatus::Previous),
        v1::CredentialStatus::CREDENTIAL_STATUS_REVOKED => Ok(CredentialStatus::Revoked),
        v1::CredentialStatus::CREDENTIAL_STATUS_EXPIRED => Ok(CredentialStatus::Expired),
        v1::CredentialStatus::CREDENTIAL_STATUS_DESTROYED => Ok(CredentialStatus::Destroyed),
        v1::CredentialStatus::CREDENTIAL_STATUS_UNSPECIFIED => Err(invalid_field(
            field,
            CredentialFieldInvalidError::UnspecifiedEnum(UnspecifiedEnumKind::CredentialStatus),
        )),
    }
}

pub(crate) fn credential_metadata_to_proto_state(value: &CredentialMetadata) -> state_v1::CredentialMetadata {
    state_v1::CredentialMetadata {
        reference: MessageField::some(credential_ref_to_proto(value.reference())),
        status: Some(proto_credential_status(value.status()).into()),
        storage_backend: Some(proto_state_storage_backend(value.storage_backend()).into()),
        fingerprint: value.fingerprint().as_str().to_string(),
    }
}

pub(crate) fn decode_credential_metadata_state(
    field: impl Into<CredentialFieldPath>,
    value: &state_v1::CredentialMetadata,
) -> Result<CredentialMetadata, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    Ok(CredentialMetadata::new(
        decode_credential_ref(
            field.child("reference"),
            decode_message_field(field.child("reference"), &value.reference)?,
        )?,
        decode_credential_status(field.child("status"), value.status.as_ref())?,
        decode_storage_backend_state(field.child("storage_backend"), value.storage_backend.as_ref())?,
        CredentialFingerprint::new(&value.fingerprint)
            .map_err(|source| invalid_field(field.child("fingerprint"), source))?,
    ))
}

pub(crate) fn pending_write_to_proto_state(
    credential_id: &CredentialId,
    scope: &CredentialScope,
    kind: CredentialKind,
) -> state_v1::PendingCredentialWriteState {
    state_v1::PendingCredentialWriteState {
        credential_id: credential_id.as_str().to_string(),
        owner_id: scope.owner_id().as_str().to_string(),
        source: Some(proto_source_kind(scope.source_kind()).into()),
        kind: Some(proto_credential_kind(kind).into()),
        scope_key: scope.scope_key(),
    }
}

pub(crate) fn decode_pending_write_state(
    state: &state_v1::PendingCredentialWriteState,
) -> Result<(CredentialId, CredentialScope, CredentialKind), CredentialProtoDecodeError> {
    let credential_id = decode_credential_id("pending_write.credential_id", &state.credential_id)?;
    let owner_id = decode_owner_id("pending_write.owner_id", &state.owner_id)?;
    let source = decode_source_kind("pending_write.source", state.source.as_ref())?;
    let kind = decode_credential_kind("pending_write.kind", state.kind.as_ref())?;
    let scope = decode_scope_key(owner_id, source, "pending_write.scope_key", &state.scope_key)?;
    Ok((credential_id, scope, kind))
}

pub(crate) fn decode_active_state(
    field: impl Into<CredentialFieldPath>,
    value: &state_v1::ActiveCredentialState,
) -> Result<(CredentialMetadata, Vec<CredentialRef>), CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let metadata = decode_credential_metadata_state(
        field.child("metadata"),
        decode_message_field(field.child("metadata"), &value.metadata)?,
    )?;
    let previous_versions = value
        .previous_versions
        .iter()
        .map(|credential| decode_credential_ref(field.child("previous_versions"), credential))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((metadata, previous_versions))
}

pub(crate) fn active_state_to_proto(
    metadata: &CredentialMetadata,
    previous_versions: &[CredentialRef],
) -> state_v1::ActiveCredentialState {
    state_v1::ActiveCredentialState {
        metadata: MessageField::some(credential_metadata_to_proto_state(metadata)),
        previous_versions: previous_versions.iter().map(credential_ref_to_proto).collect(),
    }
}

pub(crate) fn active_credential_ref(
    active: &state_v1::ActiveCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    let metadata = decode_message_field("active.metadata", &active.metadata)?;
    let reference = decode_message_field("active.metadata.reference", &metadata.reference)?;
    decode_credential_ref("active.metadata.reference", reference)
}

pub(crate) fn rotation_pending_to_proto_state(
    active: state_v1::ActiveCredentialState,
) -> state_v1::RotationPendingCredentialState {
    state_v1::RotationPendingCredentialState {
        active: MessageField::some(active),
    }
}

pub(crate) fn decode_write_failed_state(
    state: &state_v1::FailedCredentialWriteState,
) -> Result<(CredentialId, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_id("write_failed.credential_id", &state.credential_id)?,
        CredentialFailureReason::new(&state.reason).map_err(|source| invalid_field("write_failed.reason", source))?,
    ))
}

pub(crate) fn decode_revoked_state(
    state: &state_v1::RevokedCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_credential_ref(
        "revoked.credential_ref",
        decode_message_field("revoked.credential_ref", &state.credential_ref)?,
    )
}

pub(crate) fn decode_destroy_requested_state(
    state: &state_v1::DestroyRequestedCredentialState,
) -> Result<(CredentialRef, CredentialDestroyReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_ref(
            "destroy_requested.credential_ref",
            decode_message_field("destroy_requested.credential_ref", &state.credential_ref)?,
        )?,
        CredentialDestroyReason::new(&state.reason)
            .map_err(|source| invalid_field("destroy_requested.reason", source))?,
    ))
}

pub(crate) fn decode_destroyed_state(
    state: &state_v1::DestroyedCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_credential_ref(
        "destroyed.credential_ref",
        decode_message_field("destroyed.credential_ref", &state.credential_ref)?,
    )
}

pub(crate) fn decode_cleanup_failed_state(
    state: &state_v1::CleanupFailedCredentialState,
) -> Result<(CredentialRef, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_credential_ref(
            "cleanup_failed.credential_ref",
            decode_message_field("cleanup_failed.credential_ref", &state.credential_ref)?,
        )?,
        CredentialFailureReason::new(&state.reason).map_err(|source| invalid_field("cleanup_failed.reason", source))?,
    ))
}

pub(crate) fn write_failed_to_proto_state(
    credential_id: &CredentialId,
    reason: &CredentialFailureReason,
) -> state_v1::FailedCredentialWriteState {
    state_v1::FailedCredentialWriteState {
        credential_id: credential_id.as_str().to_string(),
        reason: reason.as_str().to_string(),
    }
}

pub(crate) fn revoked_to_proto_state(credential_ref: &CredentialRef) -> state_v1::RevokedCredentialState {
    state_v1::RevokedCredentialState {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
    }
}

pub(crate) fn destroy_requested_to_proto_state(
    credential_ref: &CredentialRef,
    reason: &CredentialDestroyReason,
) -> state_v1::DestroyRequestedCredentialState {
    state_v1::DestroyRequestedCredentialState {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
        reason: reason.as_str().to_string(),
    }
}

pub(crate) fn destroyed_to_proto_state(credential_ref: &CredentialRef) -> state_v1::DestroyedCredentialState {
    state_v1::DestroyedCredentialState {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
    }
}

pub(crate) fn cleanup_failed_to_proto_state(
    credential_ref: &CredentialRef,
    reason: &CredentialFailureReason,
) -> state_v1::CleanupFailedCredentialState {
    state_v1::CleanupFailedCredentialState {
        credential_ref: MessageField::some(credential_ref_to_proto(credential_ref)),
        reason: reason.as_str().to_string(),
    }
}

fn proto_state_storage_backend(value: StorageBackend) -> state_v1::StorageBackend {
    match value {
        StorageBackend::InMemory => state_v1::StorageBackend::STORAGE_BACKEND_IN_MEMORY,
        StorageBackend::OpenBao => state_v1::StorageBackend::STORAGE_BACKEND_OPENBAO,
        StorageBackend::StaticConfig => state_v1::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG,
    }
}

fn decode_storage_backend_state(
    field: impl Into<CredentialFieldPath>,
    value: Option<&EnumValue<state_v1::StorageBackend>>,
) -> Result<StorageBackend, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    match decode_known_enum(&field, value)? {
        state_v1::StorageBackend::STORAGE_BACKEND_IN_MEMORY => Ok(StorageBackend::InMemory),
        state_v1::StorageBackend::STORAGE_BACKEND_OPENBAO => Ok(StorageBackend::OpenBao),
        state_v1::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG => Ok(StorageBackend::StaticConfig),
        state_v1::StorageBackend::STORAGE_BACKEND_UNSPECIFIED => Err(invalid_field(
            field,
            CredentialFieldInvalidError::UnspecifiedEnum(UnspecifiedEnumKind::StorageBackend),
        )),
    }
}

pub(crate) fn decode_message_field<T: Default, P: ProtoBox<T>>(
    field: impl Into<CredentialFieldPath>,
    value: &MessageField<T, P>,
) -> Result<&T, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    value
        .as_option()
        .ok_or(CredentialProtoDecodeError::MissingField { field })
}

pub(crate) fn decode_credential_id(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialId, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    CredentialId::new(value).map_err(|source| invalid_field(field, source))
}

pub(crate) fn decode_owner_id(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialOwnerId, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    CredentialOwnerId::new(value).map_err(|source| invalid_field(field, source))
}

pub(crate) fn decode_scope_key(
    owner_id: CredentialOwnerId,
    source: SourceKind,
    field: impl Into<CredentialFieldPath>,
    scope_key: &str,
) -> Result<CredentialScope, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    if scope_key.is_empty() {
        return Err(invalid_field(&field, CredentialFieldInvalidError::EmptyScopeKey));
    }

    if scope_key == source.as_str() {
        return Ok(CredentialScope::source(owner_id, source));
    }

    let Some(integration_id) = scope_key.strip_prefix(&format!("{}/", source.as_str())) else {
        return Err(invalid_field(
            &field,
            CredentialFieldInvalidError::ScopeKeyMismatch {
                scope_key: scope_key.to_string(),
                source_kind: source.as_str(),
            },
        ));
    };
    let integration_id = SourceIntegrationId::new(integration_id).map_err(|source| invalid_field(&field, source))?;

    Ok(CredentialScope::integration(owner_id, source, integration_id))
}

pub(crate) fn decode_known_enum<T>(
    field: impl Into<CredentialFieldPath>,
    value: Option<&EnumValue<T>>,
) -> Result<T, CredentialProtoDecodeError>
where
    T: buffa::Enumeration,
{
    let field: CredentialFieldPath = field.into();
    let value = value.ok_or_else(|| CredentialProtoDecodeError::MissingField { field: field.clone() })?;
    value
        .as_known()
        .ok_or_else(|| invalid_field(field, CredentialFieldInvalidError::UnknownEnumValue(value.to_i32())))
}

pub(crate) fn invalid_field(
    field: impl Into<CredentialFieldPath>,
    source: impl Into<CredentialFieldInvalidError>,
) -> CredentialProtoDecodeError {
    CredentialProtoDecodeError::InvalidField {
        field: field.into(),
        reason: source.into(),
    }
}

#[cfg(test)]
mod tests;
