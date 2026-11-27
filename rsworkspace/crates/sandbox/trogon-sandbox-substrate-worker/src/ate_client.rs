//! The one place a `tonic::Status` is interpreted.
//!
//! Everything above this module sees [`AteError`], which says what to *do* about a
//! failure rather than which gRPC code produced it. Keeping the classification in a single
//! function is what makes the failure table in the integration design testable without a
//! cluster, and what stops "is `FAILED_PRECONDITION` retryable here?" from being answered
//! differently in five call sites.

mod ate_error;
mod ate_result;
mod bearer_auth;
mod bearer_token_file;
mod connect_error;
mod connect_security;
mod control;
mod resource_revision;
mod tls_trust;
mod tonic_control;

pub use ate_error::AteError;
pub use ate_result::AteResult;
pub use bearer_token_file::BearerTokenFile;
pub use connect_error::ConnectError;
pub use connect_security::ConnectSecurity;
pub use control::Control;
pub use resource_revision::ResourceRevision;
pub use tls_trust::TlsTrust;
pub use tonic_control::TonicControl;

#[cfg(test)]
mod tests;
