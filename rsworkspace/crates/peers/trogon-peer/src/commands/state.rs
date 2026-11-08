use buffa::MessageField;
use trogonai_proto::peers::{PeerEventCase, state_v1alpha1::PeerState, v1alpha1 as proto};

pub use super::evolve_error::EvolveError;
pub(super) use super::state_values::{
    parse_enrollment, read_time, required, timestamp_seconds_supported, validate_attachment,
};

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
            let generation = attachment
                .attachment_ref
                .generation
                .ok_or(EvolveError::InvalidAttachment)?;
            if let Some(previous) = state.attachment.as_option() {
                validate_attachment(enrollment, previous)?;
                let previous_observed = read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
                let previous_expires = read_time(previous.expires_at.as_option(), "attachment.expires_at")?;
                let previous_generation = previous
                    .attachment_ref
                    .generation
                    .ok_or(EvolveError::InvalidAttachment)?;
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
            state.condition = None;
        }
        PeerEventCase::PeerHeartbeatRecorded(event) => {
            let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
            let previous = state.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
            let attachment = event.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
            validate_attachment(enrollment, previous)?;
            validate_attachment(enrollment, attachment)?;
            let observed = read_time(attachment.observed_at.as_option(), "attachment.observed_at")?;
            let previous_observed = read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
            let previous_expires = read_time(previous.expires_at.as_option(), "attachment.expires_at")?;
            if previous.attachment_ref != attachment.attachment_ref
                || previous.status.and_then(|status| status.as_known())
                    != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
                || attachment.status != previous.status
                || observed < previous_observed
                || observed >= previous_expires
            {
                return Err(EvolveError::InvalidAttachment);
            }
            let availability = event
                .availability
                .and_then(|value| value.as_known())
                .ok_or(EvolveError::InvalidObservation)?;
            let condition = event
                .condition
                .map(|value| value.as_known().ok_or(EvolveError::InvalidObservation))
                .transpose()?;
            let active_at = event
                .active_at
                .as_option()
                .map(|at| read_time(Some(at), "active_at"))
                .transpose()?;
            super::domain::PeerObservation::new(availability, condition, active_at)
                .map_err(EvolveError::InvalidObservationValue)?;
            let previous_active = state
                .active_at
                .as_option()
                .map(|at| read_time(Some(at), "active_at"))
                .transpose()?;
            if active_at.is_some_and(|at| at > observed)
                || previous_active.is_some_and(|old| active_at.is_none_or(|at| at < old))
            {
                return Err(EvolveError::InvalidObservation);
            }
            state.attachment = MessageField::some(attachment.clone());
            state.availability = event.availability;
            state.condition = event.condition;
            state.active_at = event.active_at.clone();
        }
        PeerEventCase::PeerDetached(event) => {
            let enrollment = state.enrollment.as_option().ok_or(EvolveError::MissingEnrollment)?;
            let previous = state.attachment.as_option().ok_or(EvolveError::InvalidAttachment)?;
            validate_attachment(enrollment, previous)?;
            let at = read_time(event.detached_at.as_option(), "attachment.detached_at")?;
            let observed = read_time(previous.observed_at.as_option(), "attachment.observed_at")?;
            if previous.status.and_then(|status| status.as_known())
                != Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
                || event.attachment_ref != previous.attachment_ref
                || at < observed
            {
                return Err(EvolveError::InvalidAttachment);
            }
            state.attachment = MessageField::some(proto::PeerAttachment {
                attachment_ref: previous.attachment_ref.clone(),
                status: Some(proto::AttachmentStatus::ATTACHMENT_STATUS_DETACHED.into()),
                observed_at: event.detached_at.clone(),
                expires_at: event.detached_at.clone(),
            });
        }
        PeerEventCase::PeerMessageAccepted(event) => {
            super::message_rules::fold_accepted(&mut state, event)?;
        }
        PeerEventCase::PeerMessageIntakeRecorded(event) => super::message_rules::fold_intake(&mut state, event)?,
        PeerEventCase::PeerMessageDisposed(event) => super::message_rules::fold_disposed(&mut state, event)?,
        PeerEventCase::PeerBatchPrepared(event) => super::message_rules::fold_prepared(&mut state, event)?,
        _ => return Err(EvolveError::UnsupportedEvent),
    }
    Ok(state)
}
