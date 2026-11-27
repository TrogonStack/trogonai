/// A list of hosts is not a set [`InjectHosts`](crate::InjectHosts) accepts.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum InjectHostsError {
    #[error("inject_hosts must contain at least one host")]
    Empty,
}
