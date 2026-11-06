#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod activation_audit;
mod candidate;
mod certificate_validation;
pub mod constants;
mod identity_config;
mod identity_driver;
mod identity_store;
mod material_error;
mod material_authority;
mod peer_evidence;
mod registration;
mod time_observation;
mod tls;
mod tls_guards;
mod unix_endpoint;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(test)]
mod tests;

pub use activation_audit::{ActivationAudit, ActivationAuditError, ActivationFact};
pub use identity_config::IdentityConfig;
pub use identity_store::{IdentityDriver, IdentityStore, MaterialLease};
pub use material_error::MaterialError;
pub use peer_evidence::PeerEvidence;
pub use registration::{ApplicationId, RegistrationBinding, RegistrationError, RegistrationSet, WorkloadRole};
pub use unix_endpoint::{UnixEndpoint, UnixEndpointError};
