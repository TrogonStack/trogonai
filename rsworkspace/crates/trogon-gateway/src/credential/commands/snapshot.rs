use std::convert::Infallible;
use std::num::NonZeroU64;

use buffa::{EnumValue, Message as _, MessageField};
use trogon_decider_runtime::{
    FrequencySnapshot, InvalidSnapshotTypeName, SnapshotPayloadData, SnapshotPayloadDecode, SnapshotPayloadEncode,
    SnapshotType, SnapshotTypeName,
};
use trogonai_proto::gateway::credentials::state_v1 as proto_state;
use trogonai_proto::gateway::credentials::state_v1::__buffa::oneof::credential_state_snapshot::State as CredentialStateSnapshotCase;

use super::domain::credential_event::{
    decode_credential_id, decode_known_enum, decode_message_field, decode_owner_id, decode_payload, decode_scope_key,
    invalid_field, nested_field,
};
use super::domain::{
    CredentialEventPayloadError, CredentialFailureReason, CredentialFingerprint, CredentialKind, CredentialMetadata,
    CredentialRef, CredentialStatus, CredentialVersion, SourceKind, StorageBackend,
};
use super::state::{
    ActiveCredential, CredentialState, FailedCredentialWrite, PendingCredentialWrite, RevokedCredential,
    RotationPendingCredential,
};

const CREDENTIAL_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();
pub(crate) const CREDENTIAL_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(CREDENTIAL_SNAPSHOT_EVERY);

impl SnapshotType for CredentialState {
    type Error = InvalidSnapshotTypeName;

    fn snapshot_type() -> Result<SnapshotTypeName, Self::Error> {
        SnapshotTypeName::new(<proto_state::CredentialStateSnapshot as buffa::MessageName>::FULL_NAME)
    }
}

impl SnapshotPayloadEncode for CredentialState {
    type Error = Infallible;

    fn encode(&self) -> Result<Vec<u8>, Self::Error> {
        Ok(credential_state_to_proto(self).encode_to_vec())
    }
}

impl SnapshotPayloadDecode for CredentialState {
    type Error = CredentialEventPayloadError;

    fn decode(payload: SnapshotPayloadData<'_>) -> Result<Self, Self::Error> {
        decode_payload::<proto_state::CredentialStateSnapshot>(payload.payload)
            .and_then(|snapshot| credential_state_from_proto("state", &snapshot))
    }
}

fn credential_state_to_proto(value: &CredentialState) -> proto_state::CredentialStateSnapshot {
    let state = match value {
        CredentialState::Missing => proto_state::CredentialMissingState::default().into(),
        CredentialState::PendingWrite(pending) => proto_state::PendingCredentialWriteState {
            credential_id: pending.credential_id().as_str().to_string(),
            owner_id: pending.owner_id().as_str().to_string(),
            source: Some(proto_state_source_kind(pending.source()).into()),
            kind: Some(proto_state_credential_kind(pending.kind()).into()),
        }
        .into(),
        CredentialState::Active(active) => active_state_to_proto(active).into(),
        CredentialState::WriteFailed(failed) => proto_state::FailedCredentialWriteState {
            credential_id: failed.credential_id.as_str().to_string(),
            reason: failed.reason.as_str().to_string(),
        }
        .into(),
        CredentialState::RotationPending(rotation) => proto_state::RotationPendingCredentialState {
            active: MessageField::some(active_state_to_proto(rotation.active())),
        }
        .into(),
        CredentialState::Revoked(revoked) => proto_state::RevokedCredentialState {
            credential_ref: MessageField::some(credential_ref_to_proto_state(revoked.credential_ref())),
        }
        .into(),
    };

    proto_state::CredentialStateSnapshot { state: Some(state) }
}

fn active_state_to_proto(value: &ActiveCredential) -> proto_state::ActiveCredentialState {
    proto_state::ActiveCredentialState {
        metadata: MessageField::some(credential_metadata_to_proto_state(value.metadata())),
        previous_versions: value
            .previous_versions()
            .iter()
            .map(credential_ref_to_proto_state)
            .collect(),
    }
}

fn credential_state_from_proto(
    field: &'static str,
    value: &proto_state::CredentialStateSnapshot,
) -> Result<CredentialState, CredentialEventPayloadError> {
    let state = value
        .state
        .as_ref()
        .ok_or(CredentialEventPayloadError::MissingField { field })?;

    match state {
        CredentialStateSnapshotCase::Missing(_) => Ok(CredentialState::Missing),
        CredentialStateSnapshotCase::PendingWrite(pending) => {
            Ok(CredentialState::PendingWrite(PendingCredentialWrite {
                credential_id: decode_credential_id("pending_write.credential_id", &pending.credential_id)?,
                owner_id: decode_owner_id("pending_write.owner_id", &pending.owner_id)?,
                source: decode_source_kind_state("pending_write.source", pending.source.as_ref())?,
                kind: decode_credential_kind_state("pending_write.kind", pending.kind.as_ref())?,
            }))
        }
        CredentialStateSnapshotCase::Active(active) => {
            Ok(CredentialState::Active(active_state_from_proto("active", active)?))
        }
        CredentialStateSnapshotCase::WriteFailed(failed) => Ok(CredentialState::WriteFailed(FailedCredentialWrite {
            credential_id: decode_credential_id("write_failed.credential_id", &failed.credential_id)?,
            reason: CredentialFailureReason::new(&failed.reason)
                .map_err(|source| invalid_field("write_failed.reason", source))?,
        })),
        CredentialStateSnapshotCase::RotationPending(rotation) => {
            let active = decode_message_field("rotation_pending.active", &rotation.active)?;
            Ok(CredentialState::RotationPending(RotationPendingCredential {
                active: active_state_from_proto("rotation_pending.active", active)?,
            }))
        }
        CredentialStateSnapshotCase::Revoked(revoked) => {
            let credential_ref = decode_message_field("revoked.credential_ref", &revoked.credential_ref)?;
            Ok(CredentialState::Revoked(RevokedCredential {
                credential_ref: decode_credential_ref_state("revoked.credential_ref", credential_ref)?,
            }))
        }
    }
}

fn active_state_from_proto(
    field: &'static str,
    value: &proto_state::ActiveCredentialState,
) -> Result<ActiveCredential, CredentialEventPayloadError> {
    let metadata = decode_credential_metadata_state(
        nested_field(field, "metadata"),
        decode_message_field(nested_field(field, "metadata"), &value.metadata)?,
    )?;
    let previous_versions = value
        .previous_versions
        .iter()
        .map(|credential| decode_credential_ref_state("previous_versions", credential))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ActiveCredential {
        metadata,
        previous_versions,
    })
}

fn credential_metadata_to_proto_state(value: &CredentialMetadata) -> proto_state::CredentialMetadata {
    proto_state::CredentialMetadata {
        reference: MessageField::some(credential_ref_to_proto_state(value.reference())),
        status: Some(proto_state_credential_status(value.status()).into()),
        storage_backend: Some(proto_state_storage_backend(value.storage_backend()).into()),
        fingerprint: value.fingerprint().as_str().to_string(),
    }
}

fn credential_ref_to_proto_state(value: &CredentialRef) -> proto_state::CredentialRef {
    proto_state::CredentialRef {
        id: value.id().as_str().to_string(),
        version: Some(value.version().get()),
        owner_id: value.owner_id().as_str().to_string(),
        source: Some(proto_state_source_kind(value.source()).into()),
        scope_key: value.scope_key().to_string(),
        kind: Some(proto_state_credential_kind(value.kind()).into()),
    }
}

fn proto_state_source_kind(value: SourceKind) -> proto_state::CredentialSource {
    match value {
        SourceKind::Discord => proto_state::CredentialSource::CREDENTIAL_SOURCE_DISCORD,
        SourceKind::GitHub => proto_state::CredentialSource::CREDENTIAL_SOURCE_GITHUB,
        SourceKind::Gitlab => proto_state::CredentialSource::CREDENTIAL_SOURCE_GITLAB,
        SourceKind::Incidentio => proto_state::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO,
        SourceKind::Linear => proto_state::CredentialSource::CREDENTIAL_SOURCE_LINEAR,
        SourceKind::MicrosoftGraph => proto_state::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH,
        SourceKind::Notion => proto_state::CredentialSource::CREDENTIAL_SOURCE_NOTION,
        SourceKind::Sentry => proto_state::CredentialSource::CREDENTIAL_SOURCE_SENTRY,
        SourceKind::Slack => proto_state::CredentialSource::CREDENTIAL_SOURCE_SLACK,
        SourceKind::Telegram => proto_state::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM,
        SourceKind::Twitter => proto_state::CredentialSource::CREDENTIAL_SOURCE_TWITTER,
    }
}

fn proto_state_credential_kind(value: CredentialKind) -> proto_state::CredentialKind {
    match value {
        CredentialKind::AppToken => proto_state::CredentialKind::CREDENTIAL_KIND_APP_TOKEN,
        CredentialKind::BotToken => proto_state::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN,
        CredentialKind::ClientSecret => proto_state::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET,
        CredentialKind::ClientState => proto_state::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE,
        CredentialKind::ConsumerSecret => proto_state::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET,
        CredentialKind::SigningSecret => proto_state::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET,
        CredentialKind::SigningToken => proto_state::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN,
        CredentialKind::VerificationToken => proto_state::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN,
        CredentialKind::WebhookSecret => proto_state::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET,
    }
}

fn proto_state_credential_status(value: CredentialStatus) -> proto_state::CredentialStatus {
    match value {
        CredentialStatus::Pending => proto_state::CredentialStatus::CREDENTIAL_STATUS_PENDING,
        CredentialStatus::Active => proto_state::CredentialStatus::CREDENTIAL_STATUS_ACTIVE,
        CredentialStatus::Previous => proto_state::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS,
        CredentialStatus::Revoked => proto_state::CredentialStatus::CREDENTIAL_STATUS_REVOKED,
        CredentialStatus::Expired => proto_state::CredentialStatus::CREDENTIAL_STATUS_EXPIRED,
    }
}

fn proto_state_storage_backend(value: StorageBackend) -> proto_state::StorageBackend {
    match value {
        StorageBackend::InMemory => proto_state::StorageBackend::STORAGE_BACKEND_IN_MEMORY,
        StorageBackend::OpenBao => proto_state::StorageBackend::STORAGE_BACKEND_OPENBAO,
        StorageBackend::StaticConfig => proto_state::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG,
    }
}

fn decode_credential_metadata_state(
    field: &'static str,
    value: &proto_state::CredentialMetadata,
) -> Result<CredentialMetadata, CredentialEventPayloadError> {
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

fn decode_credential_ref_state(
    field: &'static str,
    value: &proto_state::CredentialRef,
) -> Result<CredentialRef, CredentialEventPayloadError> {
    let id = decode_credential_id(nested_field(field, "id"), &value.id)?;
    let owner_id = decode_owner_id(nested_field(field, "owner_id"), &value.owner_id)?;
    let source = decode_source_kind_state(nested_field(field, "source"), value.source.as_ref())?;
    let kind = decode_credential_kind_state(nested_field(field, "kind"), value.kind.as_ref())?;
    let version = value
        .version
        .ok_or(CredentialEventPayloadError::MissingField {
            field: nested_field(field, "version"),
        })
        .and_then(|value| {
            CredentialVersion::new(value).map_err(|source| invalid_field(nested_field(field, "version"), source))
        })?;
    let scope = decode_scope_key(owner_id, source, nested_field(field, "scope_key"), &value.scope_key)?;
    Ok(CredentialRef::new(id, version, &scope, kind))
}

fn decode_source_kind_state(
    field: &'static str,
    value: Option<&EnumValue<proto_state::CredentialSource>>,
) -> Result<SourceKind, CredentialEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto_state::CredentialSource::CREDENTIAL_SOURCE_DISCORD => Ok(SourceKind::Discord),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_GITHUB => Ok(SourceKind::GitHub),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_GITLAB => Ok(SourceKind::Gitlab),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_INCIDENTIO => Ok(SourceKind::Incidentio),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_LINEAR => Ok(SourceKind::Linear),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_MICROSOFT_GRAPH => Ok(SourceKind::MicrosoftGraph),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_NOTION => Ok(SourceKind::Notion),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_SENTRY => Ok(SourceKind::Sentry),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_SLACK => Ok(SourceKind::Slack),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_TELEGRAM => Ok(SourceKind::Telegram),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_TWITTER => Ok(SourceKind::Twitter),
        proto_state::CredentialSource::CREDENTIAL_SOURCE_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified source kind"))
        }
    }
}

fn decode_credential_kind_state(
    field: &'static str,
    value: Option<&EnumValue<proto_state::CredentialKind>>,
) -> Result<CredentialKind, CredentialEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto_state::CredentialKind::CREDENTIAL_KIND_APP_TOKEN => Ok(CredentialKind::AppToken),
        proto_state::CredentialKind::CREDENTIAL_KIND_BOT_TOKEN => Ok(CredentialKind::BotToken),
        proto_state::CredentialKind::CREDENTIAL_KIND_CLIENT_SECRET => Ok(CredentialKind::ClientSecret),
        proto_state::CredentialKind::CREDENTIAL_KIND_CLIENT_STATE => Ok(CredentialKind::ClientState),
        proto_state::CredentialKind::CREDENTIAL_KIND_CONSUMER_SECRET => Ok(CredentialKind::ConsumerSecret),
        proto_state::CredentialKind::CREDENTIAL_KIND_SIGNING_SECRET => Ok(CredentialKind::SigningSecret),
        proto_state::CredentialKind::CREDENTIAL_KIND_SIGNING_TOKEN => Ok(CredentialKind::SigningToken),
        proto_state::CredentialKind::CREDENTIAL_KIND_VERIFICATION_TOKEN => Ok(CredentialKind::VerificationToken),
        proto_state::CredentialKind::CREDENTIAL_KIND_WEBHOOK_SECRET => Ok(CredentialKind::WebhookSecret),
        proto_state::CredentialKind::CREDENTIAL_KIND_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified credential kind"))
        }
    }
}

fn decode_credential_status_state(
    field: &'static str,
    value: Option<&EnumValue<proto_state::CredentialStatus>>,
) -> Result<CredentialStatus, CredentialEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto_state::CredentialStatus::CREDENTIAL_STATUS_PENDING => Ok(CredentialStatus::Pending),
        proto_state::CredentialStatus::CREDENTIAL_STATUS_ACTIVE => Ok(CredentialStatus::Active),
        proto_state::CredentialStatus::CREDENTIAL_STATUS_PREVIOUS => Ok(CredentialStatus::Previous),
        proto_state::CredentialStatus::CREDENTIAL_STATUS_REVOKED => Ok(CredentialStatus::Revoked),
        proto_state::CredentialStatus::CREDENTIAL_STATUS_EXPIRED => Ok(CredentialStatus::Expired),
        proto_state::CredentialStatus::CREDENTIAL_STATUS_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified credential status"))
        }
    }
}

fn decode_storage_backend_state(
    field: &'static str,
    value: Option<&EnumValue<proto_state::StorageBackend>>,
) -> Result<StorageBackend, CredentialEventPayloadError> {
    match decode_known_enum(field, value)? {
        proto_state::StorageBackend::STORAGE_BACKEND_IN_MEMORY => Ok(StorageBackend::InMemory),
        proto_state::StorageBackend::STORAGE_BACKEND_OPENBAO => Ok(StorageBackend::OpenBao),
        proto_state::StorageBackend::STORAGE_BACKEND_STATIC_CONFIG => Ok(StorageBackend::StaticConfig),
        proto_state::StorageBackend::STORAGE_BACKEND_UNSPECIFIED => {
            Err(invalid_field(field, "unspecified storage backend"))
        }
    }
}
