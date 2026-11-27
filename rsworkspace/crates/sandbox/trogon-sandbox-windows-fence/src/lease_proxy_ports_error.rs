/// Why the fence is not in place for leases.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LeaseProxyPortsError {
    #[error("the sandbox account is not provisioned")]
    NotProvisioned,
    #[error("the network filters are {state}")]
    FiltersNotInstalled { state: String },
    #[error("the fence was installed with loopback open, so a lease proxy is not the only way out")]
    LoopbackOpen,
    #[error("the network filters report no proxy port range")]
    NoPortRange,
}
