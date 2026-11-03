use chrono::{DateTime, Utc};
use trogonai_proto::peers::v1alpha1 as proto;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("peer observation is invalid")]
pub struct PeerObservationError;

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
