#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

mod activation_audit;
mod activation_audit_error;
mod activation_fact;
mod application_id;
mod bundle_roots;
mod candidate;
mod certificate_use;
mod certificate_validation;
pub mod constants;
mod identity_config;
mod identity_driver;
mod identity_store;
mod material_authority;
mod material_error;
mod peer_evidence;
mod registration_binding;
mod registration_error;
mod registration_set;
mod scope_name;
mod time_observation;
mod tls;
mod tls_guards;
mod unix_endpoint;
mod workload_id;
mod workload_identity_wire;
mod workload_project;
mod workload_request_wire;
mod workload_response_wire;
mod workload_role;
mod workload_source_error;
mod workload_stream;
mod workload_tenant;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(test)]
mod tests;

pub use activation_audit::ActivationAudit;
pub use activation_audit_error::ActivationAuditError;
pub use activation_fact::ActivationFact;
pub use application_id::ApplicationId;
pub use identity_config::IdentityConfig;
pub use identity_store::{IdentityDriver, IdentityStore, MaterialLease};
pub use material_error::MaterialError;
pub use peer_evidence::PeerEvidence;
pub use registration_binding::RegistrationBinding;
pub use registration_error::RegistrationError;
pub use registration_set::RegistrationSet;
pub use unix_endpoint::{UnixEndpoint, UnixEndpointError};
pub use workload_role::WorkloadRole;
