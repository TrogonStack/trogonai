#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod unix_endpoint;
mod activation_audit;
pub mod constants;
mod identity_config;
mod identity_store;
mod registration;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(test)]
mod tests;

pub use unix_endpoint::{UnixEndpoint, UnixEndpointError};
pub use activation_audit::{ActivationAudit, ActivationAuditError, ActivationFact};
pub use identity_config::IdentityConfig;
pub use identity_store::{IdentityDriver, IdentityStore, MaterialLease};
pub use registration::{ApplicationId, RegistrationBinding, RegistrationError, RegistrationSet, WorkloadRole};
