use crate::PeerTenant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerAccount {
    SharedFixture { tenant: PeerTenant },
    TenantAccount { tenant: PeerTenant },
}
