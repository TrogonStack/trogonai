use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
use chrono::{DateTime, Utc};
use trogonai_proto::convert::{TimestampConversionError, datetime_from_timestamp};
use trogonai_proto::peers::{PeerEventCase, state_v1alpha1::PeerState, v1alpha1 as proto};

use super::constants::ATTACHMENT_LEASE;
use super::domain::{
    CallerId, CollaborationRef, ConnectorId, EnrollmentIdentity, InstallationId, NativeSessionId, PeerDisplayName,
    PeerValueError, TenantId, TrustedPeerOwner,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EvolveError {
    #[error("peer event has no event case")]
    MissingEvent,
    #[error("peer event case is not supported by the enrollment fold")]
    UnsupportedEvent,
    #[error("peer enrollment already exists")]
    AlreadyEnrolled,
    #[error("peer attachment already exists")]
    AlreadyAttached,
    #[error("peer enrollment is missing")]
    MissingEnrollment,
    #[error("peer field '{field}' is missing")]
    MissingField { field: &'static str },
    #[error("peer field '{field}' is invalid: {source}")]
    InvalidValue {
        field: &'static str,
        #[source]
        source: PeerValueError,
    },
    #[error("peer timestamp '{field}' is invalid: {source}")]
    InvalidTimestamp {
        field: &'static str,
        #[source]
        source: TimestampConversionError,
    },
    #[error("peer timestamp '{field}' is outside the protobuf range")]
    TimestampRange { field: &'static str },
    #[error("peer identity does not match its enrollment tuple")]
    IdentityMismatch,
    #[error("peer attachment generation, status, or lease is invalid")]
    InvalidAttachment,
}

pub fn initial_state() -> PeerState {
    PeerState {
        next_intake_ordinal: Some(1),
        ..Default::default()
    }
}

pub fn evolve(mut state: PeerState, event: &proto::PeerEvent) -> Result<PeerState, EvolveError> {
    match event.event.as_ref().ok_or(EvolveError::MissingEvent)? {
        PeerEventCase::PeerEnrolled(event) => {
            if state.enrollment.is_set() {
                return Err(EvolveError::AlreadyEnrolled);
            }
            let enrollment = event.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
            parse_enrollment(enrollment)?;
            state.enrollment = MessageField::some(enrollment.clone());
        }
        PeerEventCase::PeerAttached(event) => {
            let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
            let attachment = event
                .attachment
                .as_option()
                .ok_or(EvolveError::MissingField { field: "attachment" })?;
            validate_attachment(enrollment, attachment)?;
            if attachment.status.and_then(|status| status.as_known())
                != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
            {
                return Err(EvolveError::InvalidAttachment);
            }
            let observed = read_time(attachment.observed_at.as_option(), "attachment.observed_at")?;
            let generation = attachment.attachment_ref.generation.ok_or(EvolveError::InvalidAttachment)?;
            if let Some(previous) = state.attachment.as_option() {
                validate_attachment(enrollment, previous)?;
                let previous_observed = read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
                let previous_expires = read_time(previous.expires_at.as_option(), "attachment.expires_at")?;
                let previous_generation = previous.attachment_ref.generation.ok_or(EvolveError::InvalidAttachment)?;
                if previous_generation.checked_add(1) != Some(generation) || observed < previous_observed {
                    return Err(EvolveError::InvalidAttachment);
                }
                if previous.status.and_then(|status| status.as_known())
                    == Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
                    && observed < previous_expires
                {
                    return Err(EvolveError::InvalidAttachment);
                }
            } else if generation != 1 {
                return Err(EvolveError::InvalidAttachment);
            }
            state.attachment = MessageField::some(attachment.clone());
            state.availability = Some(proto::PeerAvailability::PEER_AVAILABILITY_READY.into());
        }
        _ => return Err(EvolveError::UnsupportedEvent),
    }
    Ok(state)
}

pub(super) fn parse_enrollment(enrollment: &proto::PeerEnrollment) -> Result<EnrollmentIdentity, EvolveError> {
    let tenant = TenantId::new(required(&enrollment.tenant_id, "enrollment.tenant_id")?).map_err(|source| {
        EvolveError::InvalidValue {
            field: "enrollment.tenant_id",
            source,
        }
    })?;
    let caller = CallerId::new(required(&enrollment.caller_id, "enrollment.caller_id")?).map_err(|source| {
        EvolveError::InvalidValue {
            field: "enrollment.caller_id",
            source,
        }
    })?;
    let installation = InstallationId::new(required(&enrollment.installation_id, "enrollment.installation_id")?)
        .map_err(|source| EvolveError::InvalidValue {
            field: "enrollment.installation_id",
            source,
        })?;
    let native_session = NativeSessionId::new(required(&enrollment.native_session_id, "enrollment.native_session_id")?)
        .map_err(|source| EvolveError::InvalidValue {
            field: "enrollment.native_session_id",
            source,
        })?;
    let identity = EnrollmentIdentity::new(TrustedPeerOwner::new(tenant, caller), installation, native_session);
    if required(&enrollment.peer_id, "enrollment.peer_id")? != identity.peer_id().as_str() {
        return Err(EvolveError::IdentityMismatch);
    }
    CollaborationRef::new(required(&enrollment.collaboration_ref, "enrollment.collaboration_ref")?).map_err(
        |source| EvolveError::InvalidValue {
            field: "enrollment.collaboration_ref",
            source,
        },
    )?;
    PeerDisplayName::new(required(&enrollment.display_name, "enrollment.display_name")?).map_err(|source| {
        EvolveError::InvalidValue {
            field: "enrollment.display_name",
            source,
        }
    })?;
    read_time(enrollment.enrolled_at.as_option(), "enrollment.enrolled_at")?;
    Ok(identity)
}

pub(super) fn required<'a>(value: &'a Option<String>, field: &'static str) -> Result<&'a str, EvolveError> {
    value.as_deref().ok_or(EvolveError::MissingField { field })
}

pub(super) fn timestamp_seconds_supported(seconds: i64) -> bool {
    (-62_135_596_800..=253_402_300_799).contains(&seconds)
}

pub(super) fn read_time(value: Option<&Timestamp>, field: &'static str) -> Result<DateTime<Utc>, EvolveError> {
    let value = value.ok_or(EvolveError::MissingField { field })?;
    if !timestamp_seconds_supported(value.seconds) || !(0..1_000_000_000).contains(&value.nanos) {
        return Err(EvolveError::TimestampRange { field });
    }
    datetime_from_timestamp(value).map_err(|source| EvolveError::InvalidTimestamp { field, source })
}

pub(super) fn validate_attachment(
    enrollment: &proto::PeerEnrollment,
    attachment: &proto::PeerAttachment,
) -> Result<(), EvolveError> {
    let reference = attachment.attachment_ref.as_option().ok_or(EvolveError::MissingField {
        field: "attachment.ref",
    })?;
    if required(&reference.peer_id, "attachment.peer_id")? != required(&enrollment.peer_id, "enrollment.peer_id")? {
        return Err(EvolveError::IdentityMismatch);
    }
    ConnectorId::new(required(&reference.connector_id, "attachment.connector_id")?).map_err(|source| {
        EvolveError::InvalidValue {
            field: "attachment.connector_id",
            source,
        }
    })?;
    if reference.generation.unwrap_or(0) == 0 {
        return Err(EvolveError::InvalidAttachment);
    }
    let observed = read_time(attachment.observed_at.as_option(), "attachment.observed_at")?;
    let expires = read_time(attachment.expires_at.as_option(), "attachment.expires_at")?;
    let enrolled = read_time(enrollment.enrolled_at.as_option(), "enrollment.enrolled_at")?;
    if observed < enrolled {
        return Err(EvolveError::InvalidAttachment);
    }
    match attachment.status.and_then(|status| status.as_known()) {
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
            if expires.signed_duration_since(observed) == ATTACHMENT_LEASE =>
        {
            Ok(())
        }
        Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED) if expires == observed => Ok(()),
        _ => Err(EvolveError::InvalidAttachment),
    }
}
