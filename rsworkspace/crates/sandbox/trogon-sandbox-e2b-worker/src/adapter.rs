//! The E2B [`Provider`](trogon_sandbox_protocol::Provider): leases, authority custody and native snapshots.

mod active_lease;
mod allocation;
mod authority;
mod cleanup;
#[cfg(test)]
mod cleanup_tests;
mod constants;
mod dispatch_recovery;
mod e2b_authority_error;
mod e2b_dispatch_error;
mod e2b_provider_error;
mod e2b_worker_provider;
mod lease_access;
mod lease_lifetime;
mod lease_lifetime_error;
mod lease_views;
mod pause_ownership;
mod provider_impl;
#[cfg(test)]
mod recovery_tests;
mod snapshot_lifecycle;
mod snapshot_ops;
#[cfg(test)]
mod snapshot_ops_tests;
mod targeted_dispatch;
#[cfg(test)]
mod targeted_tests;
#[cfg(test)]
mod tests;

pub(crate) use active_lease::ActiveLease;
pub use constants::NATIVE_OPTION_KEYS;
pub use e2b_authority_error::E2bAuthorityError;
pub use e2b_dispatch_error::E2bDispatchError;
pub use e2b_provider_error::E2bProviderError;
pub use e2b_worker_provider::E2bWorkerProvider;
pub(crate) use lease_access::LeaseAccess;
pub(crate) use lease_lifetime::LeaseLifetime;
pub use lease_lifetime_error::LeaseLifetimeError;
pub(crate) use pause_ownership::PauseOwnership;
