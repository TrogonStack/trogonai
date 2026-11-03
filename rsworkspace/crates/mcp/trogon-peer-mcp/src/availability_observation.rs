use trogonai_proto::peers::v1alpha1::PeerAvailability;

#[derive(Clone, Copy, Debug)]
pub enum AvailabilityObservation {
    Ready,
    Busy,
    Blocked,
    Draining,
}

impl AvailabilityObservation {
    pub(crate) fn wire(self) -> trogonai_proto::peers::v1alpha1::PeerAvailability {
        match self {
            Self::Ready => PeerAvailability::PEER_AVAILABILITY_READY,
            Self::Busy => PeerAvailability::PEER_AVAILABILITY_BUSY,
            Self::Blocked => PeerAvailability::PEER_AVAILABILITY_BLOCKED,
            Self::Draining => PeerAvailability::PEER_AVAILABILITY_DRAINING,
        }
    }
}
