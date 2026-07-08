use std::convert::Infallible;
use std::fmt;

use buffa::{EnumValue, Message as _, MessageField};
use trogon_decider_runtime::{EventData, EventDecode, EventDecodeOutcome, EventEncode, EventIdentity, EventType};
use trogonai_proto::gateway::credentials::v1 as proto;

use super::{
    CredentialFailureReason, CredentialFingerprint, CredentialId, CredentialKind, CredentialMetadata,
    CredentialOwnerId, CredentialRef, CredentialScope, CredentialStatus, CredentialVersion, SourceKind, StorageBackend,
};
use crate::source_integration_id::SourceIntegrationId;

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

pub(crate) fn decode_message_field<'a, T: Default>(
    field: &'static str,
    value: &'a MessageField<T>,
) -> Result<&'a T, CredentialLifecycleEventPayloadError> {
    value
        .as_option()
        .ok_or(CredentialLifecycleEventPayloadError::MissingField { field })
}

pub(crate) fn decode_payload<T>(payload: &[u8]) -> Result<T, CredentialLifecycleEventPayloadError>
where
    T: buffa::Message,
{
    T::decode_from_slice(payload).map_err(CredentialLifecycleEventPayloadError::Decode)
}

pub(crate) fn proto_event_type<T>() -> &'static str
where
    T: buffa::MessageName,
{
    T::FULL_NAME
}

pub(crate) fn decode_credential_id(
    field: &'static str,
    value: &str,
) -> Result<CredentialId, CredentialLifecycleEventPayloadError> {
    CredentialId::new(value).map_err(|source| invalid_field(field, source))
}

pub(crate) fn decode_owner_id(
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

pub(crate) fn decode_scope_key(
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

pub(crate) fn decode_known_enum<T>(
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
        _ => child,
    }
}

pub(crate) fn invalid_field(field: &'static str, source: impl fmt::Display) -> CredentialLifecycleEventPayloadError {
    CredentialLifecycleEventPayloadError::InvalidField {
        field,
        reason: source.to_string(),
    }
}
