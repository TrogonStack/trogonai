/// The kernel command line names an actor id that a bound guest cannot serve.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum GuestActorIdError {
    #[error("guest actor id must not be empty")]
    Empty,
    #[error("kernel command line has no ate.actor_id= argument; pass an actor id or ate.actor_id=unbound")]
    Missing,
    #[error("guest actor id must not be the unbound marker")]
    UnboundMarker,
}
