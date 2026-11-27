/// Why a session ended on its own.
pub(crate) enum Ended {
    Detached,
    Idle,
    Expired,
    InputOverflow,
    Closed,
}

impl Ended {
    /// What the client is told, or `None` when it asked for the end itself.
    pub(crate) fn reason(&self) -> Option<&'static str> {
        match self {
            Self::Detached => None,
            Self::Idle => Some("ssh session idle timeout"),
            Self::Expired => Some("ssh session expired"),
            Self::InputOverflow => Some("ssh session input buffer exceeded"),
            Self::Closed => Some("ssh session stream closed"),
        }
    }
}
