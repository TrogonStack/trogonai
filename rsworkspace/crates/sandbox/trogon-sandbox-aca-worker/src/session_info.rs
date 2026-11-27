use crate::{SessionIdentifier, SessionStatus};

/// What the data plane reports about one session.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SessionInfo {
    pub identifier: SessionIdentifier,
    pub status: SessionStatus,
}
