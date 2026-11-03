use crate::{PeerAccount, PeerRoute, RoutingTokenError};

#[derive(Debug, Clone)]
pub struct PeerIngress {
    account: PeerAccount,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PeerIngressError {
    #[error("peer request subject is outside the configured routing contract")]
    InvalidSubject,
    #[error("peer request tenant differs from the configured account")]
    TenantMismatch,
    #[error("peer request contains an invalid principal token: {0}")]
    InvalidPrincipal(#[source] RoutingTokenError),
    #[error("peer request method is unknown")]
    UnknownMethod,
    #[error("peer routing is not implemented")]
    Unimplemented,
}

impl PeerIngress {
    pub fn new(account: PeerAccount) -> Self {
        Self { account }
    }

    pub fn account(&self) -> &PeerAccount {
        &self.account
    }

    pub fn parse(&self, _subject: &str) -> Result<PeerRoute, PeerIngressError> {
        Err(PeerIngressError::Unimplemented)
    }

    pub fn subject(&self, _route: &PeerRoute) -> Result<String, PeerIngressError> {
        Err(PeerIngressError::Unimplemented)
    }
}
