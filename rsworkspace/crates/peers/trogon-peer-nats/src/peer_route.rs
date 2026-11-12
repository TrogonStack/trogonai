use crate::{PeerMethod, PeerPrincipal};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRoute {
    principal: PeerPrincipal,
    method: PeerMethod,
}

impl PeerRoute {
    pub fn new(principal: PeerPrincipal, method: PeerMethod) -> Self {
        Self { principal, method }
    }

    pub fn principal(&self) -> &PeerPrincipal {
        &self.principal
    }

    pub fn method(&self) -> PeerMethod {
        self.method
    }
}
