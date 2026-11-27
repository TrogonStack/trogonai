//! Typed NATS client for the sandbox credential broker: issue, revoke, and rotate requests with a bounded wait
//! for each reply.

mod client_error;
mod constants;
mod credential_client;
mod issue_params;
mod registry_auth;

pub use client_error::ClientError;
pub use credential_client::CredentialClient;
pub use issue_params::IssueParams;
pub use registry_auth::RegistryAuth;
