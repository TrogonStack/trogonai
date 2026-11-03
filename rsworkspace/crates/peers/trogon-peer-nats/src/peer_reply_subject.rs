use crate::{PeerPrincipal, RoutingTokenError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReplySubject(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerReplySubjectError {
    #[error("peer response destination is outside the authenticated requester inbox")]
    OutsideRequesterInbox,
    #[error("peer response destination contains an invalid token: {0}")]
    InvalidToken(#[source] RoutingTokenError),
}

impl PeerReplySubject {
    pub fn parse_for(_principal: &PeerPrincipal, _subject: &str) -> Result<Self, PeerReplySubjectError> {
        Err(PeerReplySubjectError::OutsideRequesterInbox)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
