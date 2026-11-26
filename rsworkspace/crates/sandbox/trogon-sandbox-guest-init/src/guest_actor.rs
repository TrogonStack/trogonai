use crate::GuestActorId;

/// The actor a guest serves: either none yet, or the one the host bound it to.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum GuestActor {
    Unbound,
    Bound(GuestActorId),
}

impl GuestActor {
    /// The text the guest records for its actor: the actor id, or the unbound marker.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Unbound => GuestActorId::UNBOUND_MARKER,
            Self::Bound(actor_id) => actor_id.as_str(),
        }
    }
}
