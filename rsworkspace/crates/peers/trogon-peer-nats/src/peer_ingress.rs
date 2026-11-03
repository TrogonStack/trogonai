use crate::constants::MAX_SUBJECT_BYTES;
use crate::{PeerAccount, PeerCaller, PeerMethod, PeerPrincipal, PeerRoute, PeerTenant, RoutingTokenError};

#[derive(Debug, Clone)]
pub struct PeerIngress {
    account: PeerAccount,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PeerIngressError {
    #[error("peer routing subject exceeds 256 bytes")]
    SubjectTooLong,
    #[error("peer request subject is outside the configured routing contract")]
    InvalidSubject,
    #[error("peer request tenant differs from the configured account")]
    TenantMismatch,
    #[error("peer request contains an invalid principal token: {0}")]
    InvalidPrincipal(#[source] RoutingTokenError),
    #[error("peer request method is unknown")]
    UnknownMethod,
}

impl PeerIngress {
    pub fn new(account: PeerAccount) -> Self {
        Self { account }
    }

    pub fn account(&self) -> &PeerAccount {
        &self.account
    }

    pub fn parse(&self, subject: &str) -> Result<PeerRoute, PeerIngressError> {
        if subject.len() > MAX_SUBJECT_BYTES {
            return Err(PeerIngressError::SubjectTooLong);
        }
        let parts: Vec<_> = subject.split('.').collect();
        let (tenant, caller, terminal) = match (&self.account, parts.as_slice()) {
            (PeerAccount::SharedFixture { .. }, [tenant, "peer", "v1", caller, "PeerService", terminal]) => {
                let tenant = PeerTenant::new(*tenant).map_err(PeerIngressError::InvalidPrincipal)?;
                (tenant, *caller, *terminal)
            }
            (PeerAccount::TenantAccount { tenant }, ["peer", "v1", caller, "PeerService", terminal]) => {
                (tenant.clone(), *caller, *terminal)
            }
            _ => return Err(PeerIngressError::InvalidSubject),
        };
        if &tenant != self.account.tenant() {
            return Err(PeerIngressError::TenantMismatch);
        }
        let caller = PeerCaller::new(caller).map_err(PeerIngressError::InvalidPrincipal)?;
        let method = PeerMethod::from_terminal(terminal).ok_or(PeerIngressError::UnknownMethod)?;
        Ok(PeerRoute::new(PeerPrincipal::new(tenant, caller), method))
    }

    pub fn subject(&self, route: &PeerRoute) -> Result<String, PeerIngressError> {
        if route.principal().tenant() != self.account.tenant() {
            return Err(PeerIngressError::TenantMismatch);
        }
        let caller = route.principal().caller().as_str();
        let terminal = route.method().as_str();
        let subject = match &self.account {
            PeerAccount::SharedFixture { tenant } => {
                format!("{}.peer.v1.{caller}.PeerService.{terminal}", tenant.as_str())
            }
            PeerAccount::TenantAccount { .. } => format!("peer.v1.{caller}.PeerService.{terminal}"),
        };
        if subject.len() > MAX_SUBJECT_BYTES {
            return Err(PeerIngressError::SubjectTooLong);
        }
        Ok(subject)
    }
}
