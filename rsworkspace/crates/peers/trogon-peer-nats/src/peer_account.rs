use crate::PeerTenant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAccount {
    SharedFixture { tenant: PeerTenant },
    TenantAccount { tenant: PeerTenant },
}

impl PeerAccount {
    pub fn tenant(&self) -> &PeerTenant {
        match self {
            Self::SharedFixture { tenant } | Self::TenantAccount { tenant } => tenant,
        }
    }
}
