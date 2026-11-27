use trogon_sandbox_contract::ProxyUid;

/// A workload user that is the egress proxy uid.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("user: {user:?} is the egress proxy uid {uid}; the workload must run as a different uid")]
pub struct ProxyUidWorkloadError {
    pub user: String,
    pub uid: ProxyUid,
}
