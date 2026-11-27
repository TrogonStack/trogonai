use std::net::AddrParseError;

use trogon_sandbox_contract::LeaseId;

use crate::GuestExecError;

/// Why a guest could not be located or prepared for SSH.
#[derive(Debug, thiserror::Error)]
pub enum GuestAccessError {
    /// This worker holds no such lease.
    #[error("unknown lease {lease_id}")]
    UnknownLease {
        /// The lease asked about.
        lease_id: LeaseId,
    },
    /// The VMI address could not be read.
    #[error("{source}")]
    Address {
        /// The VM whose address was read.
        vm_name: String,
        /// Why it could not be read.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The VMI reports an address that is not an IP address.
    #[error("VMI {vm_name} reports an unusable address {address:?}: {source}")]
    UnusableAddress {
        /// The VM whose address was read.
        vm_name: String,
        /// The address it reports.
        address: String,
        /// Why it is unusable.
        #[source]
        source: AddrParseError,
    },
    /// The guest exec path failed.
    #[error(transparent)]
    Exec(#[from] GuestExecError),
}
