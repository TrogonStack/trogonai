use buffa::{EnumValue, MessageField, ProtoBox};
use trogon_credential_domain::{
    CredentialDestroyReason, CredentialFailureReason, CredentialFingerprint, CredentialId, CredentialIdentity,
    CredentialIdentityVersion, CredentialKey, CredentialKind, CredentialMetadata, CredentialOwnerId, CredentialRef,
    CredentialScope, CredentialStatus, CredentialVersion, SourceIntegrationId, SourceKind,
};
use trogonai_proto::gateway::credentials::{CredentialEventCase, state_v1, v1};

use crate::{CredentialFieldInvalidError, CredentialFieldPath, CredentialProtoDecodeError, UnspecifiedEnumKind};

pub fn identity_to_proto(identity: &CredentialIdentity) -> v1::CredentialIdentity {
    v1::CredentialIdentity {
        credential_id: identity.id().to_string(),
        owner_id: identity.owner_id().as_str().to_string(),
        source: Some(proto_source_kind(identity.source()).into()),
        scope_key: identity.scope_key(),
        kind: Some(proto_credential_kind(identity.kind()).into()),
        identity_version: Some(proto_identity_version(identity.version()).into()),
    }
}

pub fn key_to_proto(key: &CredentialKey) -> v1::CredentialKey {
    v1::CredentialKey {
        credential_id: key.credential_id().to_string(),
        owner_id: key.owner_id().as_str().to_string(),
    }
}

/// Rebuilds the identity a write request recorded. The id is read back as
/// recorded and never derived again from the slot.
pub fn decode_identity(
    field: impl Into<CredentialFieldPath>,
    value: &v1::CredentialIdentity,
) -> Result<CredentialIdentity, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let id = decode_credential_id(field.child("credential_id"), &value.credential_id)?;
    let owner_id = decode_owner_id(field.child("owner_id"), &value.owner_id)?;
    let source = decode_source_kind(field.child("source"), value.source.as_ref())?;
    let kind = decode_credential_kind(field.child("kind"), value.kind.as_ref())?;
    let version = decode_identity_version(field.child("identity_version"), value.identity_version.as_ref())?;
    let scope = decode_scope_key(owner_id, source, field.child("scope_key"), &value.scope_key)?;
    Ok(CredentialIdentity::new(id, scope, kind, version))
}

pub fn decode_key(
    field: impl Into<CredentialFieldPath>,
    value: &v1::CredentialKey,
) -> Result<CredentialKey, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let credential_id = decode_credential_id(field.child("credential_id"), &value.credential_id)?;
    let owner_id = decode_owner_id(field.child("owner_id"), &value.owner_id)?;
    Ok(CredentialKey::new(owner_id, credential_id))
}

pub fn decode_version(
    field: impl Into<CredentialFieldPath>,
    value: Option<u64>,
) -> Result<CredentialVersion, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let Some(value) = value else {
        return Err(CredentialProtoDecodeError::MissingField { field });
    };
    CredentialVersion::new(value).map_err(|source| invalid_field(field, source))
}

fn decode_identity_field<P: ProtoBox<v1::CredentialIdentity>>(
    field: impl Into<CredentialFieldPath>,
    value: &MessageField<v1::CredentialIdentity, P>,
) -> Result<CredentialIdentity, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    decode_identity(&field, decode_message_field(&field, value)?)
}

fn decode_key_field<P: ProtoBox<v1::CredentialKey>>(
    value: &MessageField<v1::CredentialKey, P>,
) -> Result<CredentialKey, CredentialProtoDecodeError> {
    decode_key("key", decode_message_field("key", value)?)
}

fn decode_fingerprint(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialFingerprint, CredentialProtoDecodeError> {
    CredentialFingerprint::new(value).map_err(|source| invalid_field(field, source))
}

fn decode_failure_reason(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialFailureReason, CredentialProtoDecodeError> {
    CredentialFailureReason::new(value).map_err(|source| invalid_field(field, source))
}

fn decode_destroy_reason(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialDestroyReason, CredentialProtoDecodeError> {
    CredentialDestroyReason::new(value).map_err(|source| invalid_field(field, source))
}

fn decode_state_ref<P: ProtoBox<v1::CredentialIdentity>>(
    field: &'static str,
    identity: &MessageField<v1::CredentialIdentity, P>,
    version: Option<u64>,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    let field = CredentialFieldPath::from(field);
    Ok(CredentialRef::new(
        decode_identity_field(field.child("identity"), identity)?,
        decode_version(field.child("version"), version)?,
    ))
}

pub fn write_requested_to_proto(identity: &CredentialIdentity) -> v1::CredentialWriteRequested {
    v1::CredentialWriteRequested {
        identity: MessageField::some(identity_to_proto(identity)),
    }
}

pub fn write_failed_to_proto(key: &CredentialKey, reason: &CredentialFailureReason) -> v1::CredentialWriteFailed {
    v1::CredentialWriteFailed {
        key: MessageField::some(key_to_proto(key)),
        reason: reason.as_str().to_string(),
    }
}

pub fn activated_to_proto(metadata: &CredentialMetadata) -> v1::CredentialActivated {
    let credential = metadata.reference();
    v1::CredentialActivated {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
        fingerprint: metadata.fingerprint().as_str().to_string(),
    }
}

pub fn rotation_requested_to_proto(credential: &CredentialRef) -> v1::CredentialRotationRequested {
    v1::CredentialRotationRequested {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
    }
}

pub fn rotation_failed_to_proto(
    credential: &CredentialRef,
    reason: &CredentialFailureReason,
) -> v1::CredentialRotationFailed {
    v1::CredentialRotationFailed {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
        reason: reason.as_str().to_string(),
    }
}

pub fn rotated_to_proto(previous: &CredentialRef, metadata: &CredentialMetadata) -> v1::CredentialRotated {
    let credential = metadata.reference();
    v1::CredentialRotated {
        key: MessageField::some(key_to_proto(&credential.key())),
        previous_version: Some(previous.version().get()),
        version: Some(credential.version().get()),
        fingerprint: metadata.fingerprint().as_str().to_string(),
    }
}

pub fn revoked_to_proto(credential: &CredentialRef) -> v1::CredentialRevoked {
    v1::CredentialRevoked {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
    }
}

pub fn destroy_requested_to_proto(
    credential: &CredentialRef,
    reason: &CredentialDestroyReason,
) -> v1::CredentialDestroyRequested {
    v1::CredentialDestroyRequested {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
        reason: reason.as_str().to_string(),
    }
}

pub fn destroyed_to_proto(credential: &CredentialRef) -> v1::CredentialDestroyed {
    v1::CredentialDestroyed {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
    }
}

pub fn destroy_failed_to_proto(
    credential: &CredentialRef,
    reason: &CredentialFailureReason,
) -> v1::CredentialDestroyFailed {
    v1::CredentialDestroyFailed {
        key: MessageField::some(key_to_proto(&credential.key())),
        version: Some(credential.version().get()),
        reason: reason.as_str().to_string(),
    }
}

pub fn decode_write_requested(
    event: &v1::CredentialWriteRequested,
) -> Result<CredentialIdentity, CredentialProtoDecodeError> {
    decode_identity_field("identity", &event.identity)
}

pub fn decode_write_failed(
    event: &v1::CredentialWriteFailed,
) -> Result<(CredentialKey, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_key_field(&event.key)?,
        decode_failure_reason("reason", &event.reason)?,
    ))
}

pub fn decode_activated(
    event: &v1::CredentialActivated,
) -> Result<(CredentialKey, CredentialVersion, CredentialFingerprint), CredentialProtoDecodeError> {
    Ok((
        decode_key_field(&event.key)?,
        decode_version("version", event.version)?,
        decode_fingerprint("fingerprint", &event.fingerprint)?,
    ))
}

pub fn decode_rotation_requested(
    event: &v1::CredentialRotationRequested,
) -> Result<(CredentialKey, CredentialVersion), CredentialProtoDecodeError> {
    Ok((decode_key_field(&event.key)?, decode_version("version", event.version)?))
}

pub fn decode_rotation_failed(
    event: &v1::CredentialRotationFailed,
) -> Result<(CredentialKey, CredentialVersion, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_key_field(&event.key)?,
        decode_version("version", event.version)?,
        decode_failure_reason("reason", &event.reason)?,
    ))
}

pub fn decode_rotated(
    event: &v1::CredentialRotated,
) -> Result<
    (
        CredentialKey,
        CredentialVersion,
        CredentialVersion,
        CredentialFingerprint,
    ),
    CredentialProtoDecodeError,
> {
    Ok((
        decode_key_field(&event.key)?,
        decode_version("previous_version", event.previous_version)?,
        decode_version("version", event.version)?,
        decode_fingerprint("fingerprint", &event.fingerprint)?,
    ))
}

pub fn decode_revoked(
    event: &v1::CredentialRevoked,
) -> Result<(CredentialKey, CredentialVersion), CredentialProtoDecodeError> {
    Ok((decode_key_field(&event.key)?, decode_version("version", event.version)?))
}

pub fn decode_destroy_requested(
    event: &v1::CredentialDestroyRequested,
) -> Result<(CredentialKey, CredentialVersion, CredentialDestroyReason), CredentialProtoDecodeError> {
    Ok((
        decode_key_field(&event.key)?,
        decode_version("version", event.version)?,
        decode_destroy_reason("reason", &event.reason)?,
    ))
}

pub fn decode_destroyed(
    event: &v1::CredentialDestroyed,
) -> Result<(CredentialKey, CredentialVersion), CredentialProtoDecodeError> {
    Ok((decode_key_field(&event.key)?, decode_version("version", event.version)?))
}

pub fn decode_destroy_failed(
    event: &v1::CredentialDestroyFailed,
) -> Result<(CredentialKey, CredentialVersion, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_key_field(&event.key)?,
        decode_version("version", event.version)?,
        decode_failure_reason("reason", &event.reason)?,
    ))
}

/// The credential an event names, read off the event itself after the whole
/// event decodes.
pub fn event_key(event: &CredentialEventCase) -> Result<CredentialKey, CredentialProtoDecodeError> {
    match event {
        CredentialEventCase::WriteRequested(inner) => Ok(decode_write_requested(inner)?.key()),
        CredentialEventCase::WriteFailed(inner) => Ok(decode_write_failed(inner)?.0),
        CredentialEventCase::Activated(inner) => Ok(decode_activated(inner)?.0),
        CredentialEventCase::RotationRequested(inner) => Ok(decode_rotation_requested(inner)?.0),
        CredentialEventCase::RotationFailed(inner) => Ok(decode_rotation_failed(inner)?.0),
        CredentialEventCase::Rotated(inner) => Ok(decode_rotated(inner)?.0),
        CredentialEventCase::Revoked(inner) => Ok(decode_revoked(inner)?.0),
        CredentialEventCase::DestroyRequested(inner) => Ok(decode_destroy_requested(inner)?.0),
        CredentialEventCase::Destroyed(inner) => Ok(decode_destroyed(inner)?.0),
        CredentialEventCase::DestroyFailed(inner) => Ok(decode_destroy_failed(inner)?.0),
    }
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

fn proto_identity_version(value: CredentialIdentityVersion) -> v1::CredentialIdentityVersion {
    match value {
        CredentialIdentityVersion::V1 => v1::CredentialIdentityVersion::CREDENTIAL_IDENTITY_VERSION_V1,
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

fn decode_identity_version(
    field: impl Into<CredentialFieldPath>,
    value: Option<&EnumValue<v1::CredentialIdentityVersion>>,
) -> Result<CredentialIdentityVersion, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    match decode_known_enum(&field, value)? {
        v1::CredentialIdentityVersion::CREDENTIAL_IDENTITY_VERSION_V1 => Ok(CredentialIdentityVersion::V1),
        v1::CredentialIdentityVersion::CREDENTIAL_IDENTITY_VERSION_UNSPECIFIED => Err(invalid_field(
            field,
            CredentialFieldInvalidError::UnspecifiedEnum(UnspecifiedEnumKind::CredentialIdentityVersion),
        )),
    }
}

pub fn pending_write_to_proto_state(identity: &CredentialIdentity) -> state_v1::PendingCredentialWriteState {
    state_v1::PendingCredentialWriteState {
        identity: MessageField::some(identity_to_proto(identity)),
    }
}

pub fn decode_pending_write_state(
    state: &state_v1::PendingCredentialWriteState,
) -> Result<CredentialIdentity, CredentialProtoDecodeError> {
    decode_identity_field("pending_write.identity", &state.identity)
}

pub fn active_state_to_proto(
    credential: &CredentialRef,
    fingerprint: &CredentialFingerprint,
    previous_versions: &[CredentialVersion],
) -> state_v1::ActiveCredentialState {
    state_v1::ActiveCredentialState {
        identity: MessageField::some(identity_to_proto(credential.identity())),
        version: Some(credential.version().get()),
        fingerprint: fingerprint.as_str().to_string(),
        previous_versions: previous_versions.iter().copied().map(CredentialVersion::get).collect(),
    }
}

/// The active version as metadata, with every version it replaced. They all
/// share the one identity the state records.
pub fn decode_active_state(
    field: impl Into<CredentialFieldPath>,
    value: &state_v1::ActiveCredentialState,
) -> Result<(CredentialMetadata, Vec<CredentialRef>), CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    let identity = decode_identity_field(field.child("identity"), &value.identity)?;
    let version = decode_version(field.child("version"), value.version)?;
    let fingerprint = decode_fingerprint(field.child("fingerprint"), &value.fingerprint)?;
    let previous_field = field.child("previous_versions");
    let mut previous_versions = Vec::with_capacity(value.previous_versions.len());
    for previous in &value.previous_versions {
        let previous = decode_version(&previous_field, Some(*previous))?;
        previous_versions.push(CredentialRef::new(identity.clone(), previous));
    }
    let credential = CredentialRef::new(identity, version);
    Ok((
        CredentialMetadata::new(credential, CredentialStatus::Active, fingerprint),
        previous_versions,
    ))
}

pub fn active_credential_ref(
    active: &state_v1::ActiveCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_state_ref("active", &active.identity, active.version)
}

pub fn rotation_pending_to_proto_state(
    active: state_v1::ActiveCredentialState,
) -> state_v1::RotationPendingCredentialState {
    state_v1::RotationPendingCredentialState {
        active: MessageField::some(active),
    }
}

pub fn write_failed_to_proto_state(
    identity: &CredentialIdentity,
    reason: &CredentialFailureReason,
) -> state_v1::FailedCredentialWriteState {
    state_v1::FailedCredentialWriteState {
        identity: MessageField::some(identity_to_proto(identity)),
        reason: reason.as_str().to_string(),
    }
}

pub fn decode_write_failed_state(
    state: &state_v1::FailedCredentialWriteState,
) -> Result<(CredentialIdentity, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_identity_field("write_failed.identity", &state.identity)?,
        decode_failure_reason("write_failed.reason", &state.reason)?,
    ))
}

pub fn revoked_to_proto_state(credential: &CredentialRef) -> state_v1::RevokedCredentialState {
    state_v1::RevokedCredentialState {
        identity: MessageField::some(identity_to_proto(credential.identity())),
        version: Some(credential.version().get()),
    }
}

pub fn decode_revoked_state(
    state: &state_v1::RevokedCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_state_ref("revoked", &state.identity, state.version)
}

pub fn destroy_requested_to_proto_state(
    credential: &CredentialRef,
    reason: &CredentialDestroyReason,
) -> state_v1::DestroyRequestedCredentialState {
    state_v1::DestroyRequestedCredentialState {
        identity: MessageField::some(identity_to_proto(credential.identity())),
        version: Some(credential.version().get()),
        reason: reason.as_str().to_string(),
    }
}

pub fn decode_destroy_requested_state(
    state: &state_v1::DestroyRequestedCredentialState,
) -> Result<(CredentialRef, CredentialDestroyReason), CredentialProtoDecodeError> {
    Ok((
        decode_state_ref("destroy_requested", &state.identity, state.version)?,
        decode_destroy_reason("destroy_requested.reason", &state.reason)?,
    ))
}

pub fn destroyed_to_proto_state(credential: &CredentialRef) -> state_v1::DestroyedCredentialState {
    state_v1::DestroyedCredentialState {
        identity: MessageField::some(identity_to_proto(credential.identity())),
        version: Some(credential.version().get()),
    }
}

pub fn decode_destroyed_state(
    state: &state_v1::DestroyedCredentialState,
) -> Result<CredentialRef, CredentialProtoDecodeError> {
    decode_state_ref("destroyed", &state.identity, state.version)
}

pub fn cleanup_failed_to_proto_state(
    credential: &CredentialRef,
    reason: &CredentialFailureReason,
) -> state_v1::CleanupFailedCredentialState {
    state_v1::CleanupFailedCredentialState {
        identity: MessageField::some(identity_to_proto(credential.identity())),
        version: Some(credential.version().get()),
        reason: reason.as_str().to_string(),
    }
}

pub fn decode_cleanup_failed_state(
    state: &state_v1::CleanupFailedCredentialState,
) -> Result<(CredentialRef, CredentialFailureReason), CredentialProtoDecodeError> {
    Ok((
        decode_state_ref("cleanup_failed", &state.identity, state.version)?,
        decode_failure_reason("cleanup_failed.reason", &state.reason)?,
    ))
}

pub fn decode_message_field<T: Default, P: ProtoBox<T>>(
    field: impl Into<CredentialFieldPath>,
    value: &MessageField<T, P>,
) -> Result<&T, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    value
        .as_option()
        .ok_or(CredentialProtoDecodeError::MissingField { field })
}

pub fn decode_credential_id(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialId, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    CredentialId::new(value).map_err(|source| invalid_field(field, source))
}

pub fn decode_owner_id(
    field: impl Into<CredentialFieldPath>,
    value: &str,
) -> Result<CredentialOwnerId, CredentialProtoDecodeError> {
    let field: CredentialFieldPath = field.into();
    CredentialOwnerId::new(value).map_err(|source| invalid_field(field, source))
}

pub fn decode_scope_key(
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

pub fn decode_known_enum<T>(
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

pub fn invalid_field(
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
