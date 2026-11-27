//! E2B control plane REST client (sandbox create, inspect, pause, resume, delete).
//!
//! There is no vendor SDK here: the calls are plain HTTPS requests authenticated with the
//! `X-API-Key` header. The key is held only inside a sensitive header value, so it never
//! shows up in `Debug` output or logs.

mod api_error;
mod auto_pause;
mod build_wire;
mod connect_request;
mod constants;
mod created_snapshot;
mod created_snapshot_wire;
mod e2b_api;
mod e2b_config;
mod e2b_config_error;
mod memory_request;
mod native_access_token;
mod native_access_token_error;
mod native_sandbox_access;
mod new_sandbox;
mod sandbox_detail;
mod sandbox_info;
mod template_wire;
#[cfg(test)]
mod tests;
mod traffic_privacy;

pub use api_error::ApiError;
pub(crate) use api_error::error_message;
pub use auto_pause::AutoPause;
pub(crate) use constants::MAX_TIMEOUT_SECS;
pub use constants::{DEFAULT_API_URL, DEFAULT_DOMAIN};
pub(crate) use created_snapshot::CreatedSnapshot;
pub use e2b_api::E2bApi;
pub use e2b_config::E2bConfig;
pub(crate) use e2b_config::validate_domain;
pub use e2b_config_error::E2bConfigError;
pub(crate) use native_access_token::NativeAccessToken;
pub use native_access_token_error::NativeAccessTokenError;
pub(crate) use native_sandbox_access::NativeSandboxAccess;
pub use new_sandbox::NewSandbox;
pub use sandbox_detail::SandboxDetail;
pub use sandbox_info::SandboxInfo;
pub(crate) use traffic_privacy::TrafficPrivacy;
