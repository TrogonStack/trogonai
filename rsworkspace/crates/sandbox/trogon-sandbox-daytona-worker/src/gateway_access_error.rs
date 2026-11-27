use trogon_sandbox_contract::LeaseId;

/// Why the worker could not get gateway access for a lease.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum GatewayAccessError {
    #[error("this worker holds no sandbox for lease {0}")]
    UnknownLease(LeaseId),
    #[error("Daytona offered no SSH access for this sandbox")]
    Unsupported,
    #[error("Daytona refused SSH access: {0}")]
    Refused(String),
}
