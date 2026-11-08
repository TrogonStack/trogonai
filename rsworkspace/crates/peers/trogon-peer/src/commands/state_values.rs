use buffa_types::google::protobuf::Timestamp;
use chrono::{DateTime, Utc};
use trogonai_proto::convert::datetime_from_timestamp;
use trogonai_proto::peers::v1alpha1 as proto;

use super::constants::ATTACHMENT_LEASE;
use super::domain::{
    CallerId, CollaborationRef, ConnectorId, EnrollmentFacts, EnrollmentIdentity, InstallationId, NativeSessionId,
    PeerDisplayName, TenantId, TrustedPeerOwner,
};
use super::evolve_error::EvolveError;

pub(super) fn parse_enrollment(enrollment: &proto::PeerEnrollment) -> Result<EnrollmentIdentity, EvolveError> {
    Ok(parse_enrollment_facts(enrollment)?.identity)
}

pub(super) fn parse_enrollment_facts(enrollment: &proto::PeerEnrollment) -> Result<EnrollmentFacts, EvolveError> {
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
    let collaboration = CollaborationRef::new(required(&enrollment.collaboration_ref, "enrollment.collaboration_ref")?)
        .map_err(|source| EvolveError::InvalidValue {
            field: "enrollment.collaboration_ref",
            source,
        })?;
    PeerDisplayName::new(required(&enrollment.display_name, "enrollment.display_name")?).map_err(|source| {
        EvolveError::InvalidValue {
            field: "enrollment.display_name",
            source,
        }
    })?;
    read_time(enrollment.enrolled_at.as_option(), "enrollment.enrolled_at")?;
    Ok(EnrollmentFacts {
        identity,
        collaboration,
    })
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
