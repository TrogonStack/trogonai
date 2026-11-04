use chrono::{DateTime, Utc};
use trogonai_proto::peers::v1alpha1 as proto;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerObservationError {
    #[error("peer availability must be specified")]
    UnspecifiedAvailability,
    #[error("peer condition must be specified when present")]
    UnspecifiedCondition,
    #[error("peer activity is outside the supported timestamp range")]
    InvalidActivity,
}

#[derive(Debug, Clone)]
pub struct PeerObservation {
    availability: proto::PeerAvailability,
    condition: Option<proto::PeerCondition>,
    active_at: Option<DateTime<Utc>>,
}

impl PeerObservation {
    pub fn new(
        availability: proto::PeerAvailability,
        condition: Option<proto::PeerCondition>,
        active_at: Option<DateTime<Utc>>,
    ) -> Result<Self, PeerObservationError> {
        if availability == proto::PeerAvailability::PEER_AVAILABILITY_UNSPECIFIED {
            return Err(PeerObservationError::UnspecifiedAvailability);
        }
        if condition == Some(proto::PeerCondition::PEER_CONDITION_UNSPECIFIED) {
            return Err(PeerObservationError::UnspecifiedCondition);
        }
        if active_at.is_some_and(|at| {
            !(-62_135_596_800..=253_402_300_799).contains(&at.timestamp())
                || at.timestamp_subsec_nanos() >= 1_000_000_000
        }) {
            return Err(PeerObservationError::InvalidActivity);
        }
        Ok(Self {
            availability,
            condition,
            active_at,
        })
    }
    pub fn availability(&self) -> proto::PeerAvailability {
        self.availability
    }
    pub fn condition(&self) -> Option<proto::PeerCondition> {
        self.condition
    }
    pub fn active_at(&self) -> Option<DateTime<Utc>> {
        self.active_at
    }
}
