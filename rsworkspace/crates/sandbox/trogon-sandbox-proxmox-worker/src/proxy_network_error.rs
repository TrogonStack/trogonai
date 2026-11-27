use std::net::Ipv4Addr;

/// The configured guest network cannot work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProxyNetworkError {
    #[error("the Proxmox bridge name is invalid")]
    Bridge,
    #[error("the guest address pool is smaller than the worker capacity")]
    PoolTooSmall,
    #[error("guest address {0} or the gateway is invalid")]
    Address(Ipv4Addr),
    #[error("the guest address pool contains {0} twice")]
    Duplicate(Ipv4Addr),
    #[error("the proxy advertised address must be reachable from a guest")]
    Advertised,
}
