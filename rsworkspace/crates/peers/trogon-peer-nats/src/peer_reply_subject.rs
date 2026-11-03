use crate::routing_token::RoutingToken;
use crate::{PeerPrincipal, RoutingTokenError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerReplySubject(String);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PeerReplySubjectError {
    #[error("peer response destination is outside the authenticated requester inbox")]
    OutsideRequesterInbox,
    #[error("peer response destination contains an invalid token: {0}")]
    InvalidToken(#[source] RoutingTokenError),
}

impl PeerReplySubject {
    pub fn parse_for(principal: &PeerPrincipal, subject: &str) -> Result<Self, PeerReplySubjectError> {
        let parts: Vec<_> = subject.split('.').collect();
        let ["_INBOX", tenant, caller, connection, nonce] = parts.as_slice() else {
            return Err(PeerReplySubjectError::OutsideRequesterInbox);
        };
        if *tenant != principal.tenant().as_str() || *caller != principal.caller().as_str() {
            return Err(PeerReplySubjectError::OutsideRequesterInbox);
        }
        RoutingToken::new(connection).map_err(PeerReplySubjectError::InvalidToken)?;
        RoutingToken::new(nonce).map_err(PeerReplySubjectError::InvalidToken)?;
        Ok(Self(subject.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}
