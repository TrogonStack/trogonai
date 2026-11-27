#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Worker that fulfills sandbox lease requests by running each lease in an Azure Container Apps
//! Dynamic Sessions session.

mod aca_authority_host;
mod aca_error;
mod aca_session_api;
mod aca_worker_provider;
mod active_aca_lease;
mod azure_volume_backends;
mod broker_credential_issuer;
mod build_heartbeat;
mod cancel_dispatch;
mod cancel_lease_id;
mod cleanup_retries;
mod constants;
mod credential_issuer;
mod credential_ttl;
mod directory_url;
mod execute_result;
mod git_clone;
mod lease_dispatch;
mod materialize_workspace;
mod materialized_workspace;
mod nats_lifecycle_emitter;
mod profile;
mod provider_config;
mod publish_reply;
mod registry_image;
mod reqwest_aca_client;
mod run_worker;
mod serve;
mod serve_error;
mod service_principal;
mod session_delete_outcome;
mod session_identifier;
mod session_info;
mod session_status;
mod token_endpoint;
mod upload_directory;
mod volume_backends;
mod volume_credential;
mod volume_support;
mod volume_sync_plan;
mod wait_session_ready;
mod worker_args;
mod worker_binding_error;
mod worker_error;
mod worker_event;

pub use aca_error::AcaError;
pub use aca_session_api::AcaSessionApi;
pub use aca_worker_provider::AcaWorkerProvider;
pub use azure_volume_backends::AzureVolumeBackends;
pub use broker_credential_issuer::BrokerCredentialIssuer;
pub use build_heartbeat::build_heartbeat;
pub use cancel_lease_id::extract_cancel_lease_id;
pub use cleanup_retries::run_cleanup_retries;
pub use constants::{
    CLEANUP_RETRY_INTERVAL, DEFAULT_API_VERSION, DEFAULT_LOGIN_ENDPOINT, DYNAMIC_SESSIONS_SCOPE, LEASE_QUEUE_GROUP,
    MAX_LEASE_LIFETIME_SECONDS,
};
pub use credential_issuer::CredentialIssuer;
pub use credential_ttl::credential_ttl_seconds;
pub use execute_result::ExecuteResult;
pub use git_clone::{authenticated_clone_url, clone_git_workspace};
pub use materialize_workspace::materialize_workspace;
pub use materialized_workspace::MaterializedWorkspace;
pub use nats_lifecycle_emitter::NatsLifecycleEmitter;
pub use profile::PROFILE;
pub use provider_config::ProviderConfig;
pub use registry_image::{image_env_vars, resolve_registry_image};
pub use reqwest_aca_client::ReqwestAcaClient;
pub use run_worker::run_worker;
pub use serve::serve;
pub use serve_error::ServeError;
pub use service_principal::ServicePrincipal;
pub use session_delete_outcome::SessionDeleteOutcome;
pub use session_identifier::SessionIdentifier;
pub use session_info::SessionInfo;
pub use session_status::SessionStatus;
pub use token_endpoint::fetch_dynamic_sessions_token;
pub use upload_directory::upload_directory;
pub use volume_backends::VolumeBackends;
pub use volume_credential::VolumeCredential;
pub use volume_support::VolumeSupport;
pub use volume_sync_plan::VolumeSyncPlan;
pub use wait_session_ready::wait_session_ready;
pub use worker_args::WorkerArgs;
pub use worker_binding_error::WorkerBindingError;
pub use worker_error::WorkerError;
