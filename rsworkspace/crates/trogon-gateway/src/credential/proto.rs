use std::fmt;

use buffa::{EnumValue, MessageField};
use trogonai_proto::gateway::credentials::{state_v1, v1};

use super::commands::domain::{
    CredentialFailureReason, CredentialFingerprint, CredentialId, CredentialKind, CredentialMetadata,
    CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion, SourceKind, StorageBackend,
};
use crate::source_integration_id::SourceIntegrationId;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum CredentialProtoDecodeError {
    #[error("credential field '{field}' is missing")]
    MissingField { field: &'static str },
    #[error("credential field '{field}' is invalid: {reason}")]
    InvalidField { field: &'static str, reason: String },
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
        storage_backend: Some(proto_storage_backend(value.storage_backend()).into()),
        fingerprint: value.fingerprint().as_str().to_string(),
    }
}

pub(crate) fn decode_credential_ref(
    field: &'static str,
    value: &v1::CredentialRef,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    let id = decode_credential_id(nested_field(field, "id"), &value.id)?;
    let owner_id = decode_owner_id(nested_field(field, "owner_id"), &value.owner_id)?;
    let source = decode_source_kind(nested_field(field, "source"), value.source.as_ref())?;
    let kind = decode_credential_kind(nested_field(field, "kind"), value.kind.as_ref())?;
    let version = value
        .version
        .ok_or(CredentialProtoDecodeError::MissingField {
            field: nested_field(field, "version"),
        })
        .and_then(|value| {
            CredentialVersion::new(value).map_err(|source| invalid_field(nested_field(field, "version"), source))
        })?;
    let scope = decode_scope_key(owner_id, source, nested_field(field, "scope_key"), &value.scope_key)?;
    Ok(CredentialRef::new(id, version, &scope, kind))
}

pub(crate) fn write_requested_to_proto(
    credential_id: &CredentialId,
    owner_id: &CredentialOwnerId,
    source: SourceKind,
    kind: CredentialKind,
) -> v1::CredentialWriteRequested {
    v1::CredentialWriteRequested {
        credential_id: credential_id.as_str().to_string(),
        owner_id: owner_id.as_str().to_string(),
        source: Some(proto_source_kind(source).into()),
        kind: Some(proto_credential_kind(kind).into()),
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

pub(crate) fn decode_write_requested(
    event: &v1::CredentialWriteRequested,
) -> Result<(CredentialId, CredentialOwnerId, SourceKind, CredentialKind), CredentialProtoDecodeError> {
    Ok((
        decode_credential_id("credential_id", &event.credential_id)?,
        decode_owner_id("owner_id", &event.owner_id)?,
        decode_source_kind("source", event.source.as_ref())?,
        decode_credential_kind("kind", event.kind.as_ref())?,
    ))
}

pub(crate) fn decode_credential_metadata(
    field: &'static str,
    value: &v1::CredentialMetadata,
) -> Result<CredentialMetadata, CredentialProtoDecodeError> {
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
    }
}

fn proto_credential_status(value: CredentialStatus) -> v1::CredentialStatus {
    match value {
        CredentialStatus::Pending => v1::CredentialStatus::CREDENTIAL_STATUS_PENDING,
        CredentialStatus::Active => v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE,
        CredentialStatus::Previous => v1::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS,
        CredentialStatus::Revoked => v1::CredentialStatus::CREDENTIAL_STATUS_REVOKED,
        CredentialStatus::Expired => v1::CredentialStatus::CREDENTIAL_STATUS_EXPIRED,
    }
}

fn proto_storage_backend(value: StorageBackend) -> v1::StorageBackend {
    match value {
        StorageBackend::InMemory => v1::StorageBackend::STORAGE_BACKEND_IN_MEMORY,
        StorageBackend::OpenBao => v1::StorageBackend::STORAGE_BACKEND_OPENBAO,
        StorageBackend::StaticConfig => v1::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG,
    }
}

fn decode_source_kind(
    field: &'static str,
    value: Option<&EnumValue<v1::CredentialSource>>,
) -> Result<SourceKind, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
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
        v1::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED => Err(invalid_field(field, "unspecified source kind")),
    }
}

fn decode_credential_kind(
    field: &'static str,
    value: Option<&EnumValue<v1::CredentialKind>>,
) -> Result<CredentialKind, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        v1::CredentialKind::CREDENTIAL_KIND_APP_TOKEN => Ok(CredentialKind::AppToken),
        v1::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN => Ok(CredentialKind::BotToken),
        v1::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET => Ok(CredentialKind::ClientSecret),
        v1::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE => Ok(CredentialKind::ClientState),
        v1::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET => Ok(CredentialKind::ConsumerSecret),
        v1::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET => Ok(CredentialKind::SigningSecret),
        v1::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN => Ok(CredentialKind::SigningToken),
        v1::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN => Ok(CredentialKind::VerificationToken),
        v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET => Ok(CredentialKind::WebhookSecret),
        v1::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED => Err(invalid_field(field, "unspecified credential kind")),
    }
}

fn decode_credential_status(
    field: &'static str,
    value: Option<&EnumValue<v1::CredentialStatus>>,
) -> Result<CredentialStatus, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        v1::CredentialStatus::CREDENTIAL_STATUS_PENDING => Ok(CredentialStatus::Pending),
        v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE => Ok(CredentialStatus::Active),
        v1::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS => Ok(CredentialStatus::Previous),
        v1::CredentialStatus::CREDENTIAL_STATUS_REVOKED => Ok(CredentialStatus::Revoked),
        v1::CredentialStatus::CREDENTIAL_STATUS_EXPIRED => Ok(CredentialStatus::Expired),
        v1::CredentialStatus::CREDENTIAL_STATUS_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified credential status"))
        }
    }
}

fn decode_storage_backend(
    field: &'static str,
    value: Option<&EnumValue<v1::StorageBackend>>,
) -> Result<StorageBackend, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        v1::StorageBackend::STORAGE_BACKEND_IN_MEMORY => Ok(StorageBackend::InMemory),
        v1::StorageBackend::STORAGE_BACKEND_OPENBAO => Ok(StorageBackend::OpenBao),
        v1::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG => Ok(StorageBackend::StaticConfig),
        v1::StorageBackend::STORAGE_BACKEND_UNSPECIFIED => Err(invalid_field(field, "unspecified storage backend")),
    }
}

pub(crate) fn credential_ref_to_proto_state(value: &CredentialRef) -> state_v1::CredentialRef {
    state_v1::CredentialRef {
        id: value.id().as_str().to_string(),
        version: Some(value.version().get()),
        owner_id: value.owner_id().as_str().to_string(),
        source: Some(proto_state_source_kind(value.source()).into()),
        scope_key: value.scope_key().to_string(),
        kind: Some(proto_state_credential_kind(value.kind()).into()),
    }
}

pub(crate) fn credential_metadata_to_proto_state(value: &CredentialMetadata) -> state_v1::CredentialMetadata {
    state_v1::CredentialMetadata {
        reference: MessageField::some(credential_ref_to_proto_state(value.reference())),
        status: Some(proto_state_credential_status(value.status()).into()),
        storage_backend: Some(proto_state_storage_backend(value.storage_backend()).into()),
        fingerprint: value.fingerprint().as_str().to_string(),
    }
}

pub(crate) fn decode_credential_ref_state(
    field: &'static str,
    value: &state_v1::CredentialRef,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    let id = decode_credential_id(nested_field(field, "id"), &value.id)?;
    let owner_id = decode_owner_id(nested_field(field, "owner_id"), &value.owner_id)?;
    let source = decode_source_kind_state(nested_field(field, "source"), value.source.as_ref())?;
    let kind = decode_credential_kind_state(nested_field(field, "kind"), value.kind.as_ref())?;
    let version = value
        .version
        .ok_or(CredentialProtoDecodeError::MissingField {
            field: nested_field(field, "version"),
        })
        .and_then(|value| {
            CredentialVersion::new(value).map_err(|source| invalid_field(nested_field(field, "version"), source))
        })?;
    let scope = decode_scope_key(owner_id, source, nested_field(field, "scope_key"), &value.scope_key)?;
    Ok(CredentialRef::new(id, version, &scope, kind))
}

pub(crate) fn decode_credential_metadata_state(
    field: &'static str,
    value: &state_v1::CredentialMetadata,
) -> Result<CredentialMetadata, CredentialProtoDecodeError> {
    Ok(CredentialMetadata::new(
        decode_credential_ref_state(
            nested_field(field, "reference"),
            decode_message_field(nested_field(field, "reference"), &value.reference)?,
        )?,
        decode_credential_status_state(nested_field(field, "status"), value.status.as_ref())?,
        decode_storage_backend_state(nested_field(field, "storage_backend"), value.storage_backend.as_ref())?,
        CredentialFingerprint::new(&value.fingerprint)
            .map_err(|source| invalid_field(nested_field(field, "fingerprint"), source))?,
    ))
}

pub(crate) fn pending_write_to_proto_state(
    credential_id: &CredentialId,
    owner_id: &CredentialOwnerId,
    source: SourceKind,
    kind: CredentialKind,
) -> state_v1::PendingCredentialWriteState {
    state_v1::PendingCredentialWriteState {
        credential_id: credential_id.as_str().to_string(),
        owner_id: owner_id.as_str().to_string(),
        source: Some(proto_state_source_kind(source).into()),
        kind: Some(proto_state_credential_kind(kind).into()),
    }
}

pub(crate) fn decode_pending_write_state(
    state: &state_v1::PendingCredentialWriteState,
) -> Result<(CredentialId, CredentialOwnerId, SourceKind, CredentialKind), CredentialProtoDecodeError> {
    Ok((
        decode_credential_id("pending_write.credential_id", &state.credential_id)?,
        decode_owner_id("pending_write.owner_id", &state.owner_id)?,
        decode_source_kind_state("pending_write.source", state.source.as_ref())?,
        decode_credential_kind_state("pending_write.kind", state.kind.as_ref())?,
    ))
}

pub(crate) fn decode_active_state(
    field: &'static str,
    value: &state_v1::ActiveCredentialState,
) -> Result<(CredentialMetadata, Vec<CredentialRef>), CredentialProtoDecodeError> {
    let metadata = decode_credential_metadata_state(
        nested_field(field, "metadata"),
        decode_message_field(nested_field(field, "metadata"), &value.metadata)?,
    )?;
    let previous_versions = value
        .previous_versions
        .iter()
        .map(|credential| decode_credential_ref_state("previous_versions", credential))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((metadata, previous_versions))
}

pub(crate) fn active_state_to_proto(
    metadata: &CredentialMetadata,
    previous_versions: &[CredentialRef],
) -> state_v1::ActiveCredentialState {
    state_v1::ActiveCredentialState {
        metadata: MessageField::some(credential_metadata_to_proto_state(metadata)),
        previous_versions: previous_versions.iter().map(credential_ref_to_proto_state).collect(),
    }
}

pub(crate) fn active_credential_ref(
    active: &state_v1::ActiveCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    let metadata = decode_message_field("active.metadata", &active.metadata)?;
    let reference = decode_message_field("active.metadata.reference", &metadata.reference)?;
    decode_credential_ref_state("active.metadata.reference", reference)
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
    decode_credential_ref_state(
        "revoked.credential_ref",
        decode_message_field("revoked.credential_ref", &state.credential_ref)?,
    )
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
        credential_ref: MessageField::some(credential_ref_to_proto_state(credential_ref)),
    }
}

fn proto_state_source_kind(value: SourceKind) -> state_v1::CredentialSource {
    match value {
        SourceKind::Discord => state_v1::CredentialSource::CREDENTIAL_SOURCE_DISCORD,
        SourceKind::GitHub => state_v1::CredentialSource::CREDENTIAL_SOURCE_GITHUB,
        SourceKind::Gitlab => state_v1::CredentialSource::CREDENTIAL_SOURCE_GITLAB,
        SourceKind::Incidentio => state_v1::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO,
        SourceKind::Linear => state_v1::CredentialSource::CREDENTIAL_SOURCE_LINEAR,
        SourceKind::MicrosoftGraph => state_v1::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH,
        SourceKind::Notion => state_v1::CredentialSource::CREDENTIAL_SOURCE_NOTION,
        SourceKind::Sentry => state_v1::CredentialSource::CREDENTIAL_SOURCE_SENTRY,
        SourceKind::Slack => state_v1::CredentialSource::CREDENTIAL_SOURCE_SLACK,
        SourceKind::Telegram => state_v1::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM,
        SourceKind::Twitter => state_v1::CredentialSource::CREDENTIAL_SOURCE_TWITTER,
    }
}

fn proto_state_credential_kind(value: CredentialKind) -> state_v1::CredentialKind {
    match value {
        CredentialKind::AppToken => state_v1::CredentialKind::CREDENTIAL_KIND_APP_TOKEN,
        CredentialKind::BotToken => state_v1::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN,
        CredentialKind::ClientSecret => state_v1::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET,
        CredentialKind::ClientState => state_v1::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE,
        CredentialKind::ConsumerSecret => state_v1::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET,
        CredentialKind::SigningSecret => state_v1::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET,
        CredentialKind::SigningToken => state_v1::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN,
        CredentialKind::VerificationToken => state_v1::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN,
        CredentialKind::WebhookSecret => state_v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET,
    }
}

fn proto_state_credential_status(value: CredentialStatus) -> state_v1::CredentialStatus {
    match value {
        CredentialStatus::Pending => state_v1::CredentialStatus::CREDENTIAL_STATUS_PENDING,
        CredentialStatus::Active => state_v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE,
        CredentialStatus::Previous => state_v1::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS,
        CredentialStatus::Revoked => state_v1::CredentialStatus::CREDENTIAL_STATUS_REVOKED,
        CredentialStatus::Expired => state_v1::CredentialStatus::CREDENTIAL_STATUS_EXPIRED,
    }
}

fn proto_state_storage_backend(value: StorageBackend) -> state_v1::StorageBackend {
    match value {
        StorageBackend::InMemory => state_v1::StorageBackend::STORAGE_BACKEND_IN_MEMORY,
        StorageBackend::OpenBao => state_v1::StorageBackend::STORAGE_BACKEND_OPENBAO,
        StorageBackend::StaticConfig => state_v1::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG,
    }
}

fn decode_source_kind_state(
    field: &'static str,
    value: Option<&EnumValue<state_v1::CredentialSource>>,
) -> Result<SourceKind, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        state_v1::CredentialSource::CREDENTIAL_SOURCE_DISCORD => Ok(SourceKind::Discord),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_GITHUB => Ok(SourceKind::GitHub),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_GITLAB => Ok(SourceKind::Gitlab),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO => Ok(SourceKind::Incidentio),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_LINEAR => Ok(SourceKind::Linear),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH => Ok(SourceKind::MicrosoftGraph),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_NOTION => Ok(SourceKind::Notion),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_SENTRY => Ok(SourceKind::Sentry),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_SLACK => Ok(SourceKind::Slack),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM => Ok(SourceKind::Telegram),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_TWITTER => Ok(SourceKind::Twitter),
        state_v1::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified source kind"))
        }
    }
}

fn decode_credential_kind_state(
    field: &'static str,
    value: Option<&EnumValue<state_v1::CredentialKind>>,
) -> Result<CredentialKind, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        state_v1::CredentialKind::CREDENTIAL_KIND_APP_TOKEN => Ok(CredentialKind::AppToken),
        state_v1::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN => Ok(CredentialKind::BotToken),
        state_v1::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET => Ok(CredentialKind::ClientSecret),
        state_v1::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE => Ok(CredentialKind::ClientState),
        state_v1::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET => Ok(CredentialKind::ConsumerSecret),
        state_v1::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET => Ok(CredentialKind::SigningSecret),
        state_v1::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN => Ok(CredentialKind::SigningToken),
        state_v1::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN => Ok(CredentialKind::VerificationToken),
        state_v1::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET => Ok(CredentialKind::WebhookSecret),
        state_v1::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified credential kind"))
        }
    }
}

fn decode_credential_status_state(
    field: &'static str,
    value: Option<&EnumValue<state_v1::CredentialStatus>>,
) -> Result<CredentialStatus, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        state_v1::CredentialStatus::CREDENTIAL_STATUS_PENDING => Ok(CredentialStatus::Pending),
        state_v1::CredentialStatus::CREDENTIAL_STATUS_ACTIVE => Ok(CredentialStatus::Active),
        state_v1::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS => Ok(CredentialStatus::Previous),
        state_v1::CredentialStatus::CREDENTIAL_STATUS_REVOKED => Ok(CredentialStatus::Revoked),
        state_v1::CredentialStatus::CREDENTIAL_STATUS_EXPIRED => Ok(CredentialStatus::Expired),
        state_v1::CredentialStatus::CREDENTIAL_STATUS_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified credential status"))
        }
    }
}

fn decode_storage_backend_state(
    field: &'static str,
    value: Option<&EnumValue<state_v1::StorageBackend>>,
) -> Result<StorageBackend, CredentialProtoDecodeError> {
    match decode_known_enum(field, value)? {
        state_v1::StorageBackend::STORAGE_BACKEND_IN_MEMORY => Ok(StorageBackend::InMemory),
        state_v1::StorageBackend::STORAGE_BACKEND_OPENBAO => Ok(StorageBackend::OpenBao),
        state_v1::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG => Ok(StorageBackend::StaticConfig),
        state_v1::StorageBackend::STORAGE_BACKEND_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified storage backend"))
        }
    }
}

pub(crate) fn decode_message_field<'a, T: Default>(
    field: &'static str,
    value: &'a MessageField<T>,
) -> Result<&'a T, CredentialProtoDecodeError> {
    value
        .as_option()
        .ok_or(CredentialProtoDecodeError::MissingField { field })
}

pub(crate) fn decode_credential_id(
    field: &'static str,
    value: &str,
) -> Result<CredentialId, CredentialProtoDecodeError> {
    CredentialId::new(value).map_err(|source| invalid_field(field, source))
}

pub(crate) fn decode_owner_id(
    field: &'static str,
    value: &str,
) -> Result<CredentialOwnerId, CredentialProtoDecodeError> {
    CredentialOwnerId::new(value).map_err(|source| invalid_field(field, source))
}

pub(crate) fn decode_scope_key(
    owner_id: CredentialOwnerId,
    source: SourceKind,
    field: &'static str,
    scope_key: &str,
) -> Result<CredentialScope, CredentialProtoDecodeError> {
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

pub(crate) fn decode_known_enum<T>(
    field: &'static str,
    value: Option<&EnumValue<T>>,
) -> Result<T, CredentialProtoDecodeError>
where
    T: buffa::Enumeration,
{
    let value = value.ok_or(CredentialProtoDecodeError::MissingField { field })?;
    value
        .as_known()
        .ok_or_else(|| invalid_field(field, format!("unknown enum value {}", value.to_i32())))
}

pub(crate) fn nested_field(parent: &'static str, child: &'static str) -> &'static str {
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
        ("active", "metadata") => "active.metadata",
        ("rotation_pending.active", "metadata") => "rotation_pending.active.metadata",
        _ => child,
    }
}

pub(crate) fn invalid_field(field: &'static str, source: impl fmt::Display) -> CredentialProtoDecodeError {
    CredentialProtoDecodeError::InvalidField {
        field,
        reason: source.to_string(),
    }
}
