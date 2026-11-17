#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Client side of the secrets service's release call.
//!
//! [`ReleaseClient`] asks the secrets service for one credential release at a
//! time on `SecretExchangeService.ResolveForRelease` (ADR 0016), inside
//! PLATFORM, on
//! `{tenant}.secrets.v1.service.{id}.SecretExchangeService.ResolveForRelease`
//! (ADR#0081 Decision 3, ADR#0082 Decision 2). The tenant comes with each
//! [`ReleaseRequest`]; the runtime service, the pinned service xkey, and the
//! wait come once with the [`ReleaseClientConfig`].
//!
//! Each call makes an xkey of its own and drops it with the call. The value
//! comes back sealed to that xkey, and the client opens it only with the
//! pinned [`ServiceXkey`] as the sender (ADR#0083 Decision 1, ADR#0033
//! Decision 5), so a value sealed by anyone else never opens.
//!
//! A [`ReleasedValue`] may be held until the earlier of the service's
//! `valid_until` and [`RELEASE_HOLD_BOUND`] after it arrived (ADR#0049). A
//! value already expired when it arrives is refused.
//!
//! Every way a call produces no value is a [`ReleaseError`], and every one
//! fails closed. [`ReleaseError::Denied`] carries nothing, so a refusal says
//! no more to the caller than the service chose to.
//!
//! # Features
//!
//! - `test-support`: adds `FakeSecretsService`, a scripted stand-in for the
//!   secrets service that seals with its own xkey and records what it was
//!   asked, for crates that test against the release call. Off by default;
//!   enable it only from `[dev-dependencies]`.

mod constants;
#[cfg(any(test, feature = "test-support"))]
mod fake_answer;
#[cfg(any(test, feature = "test-support"))]
mod fake_outcome;
#[cfg(any(test, feature = "test-support"))]
mod fake_release_handler;
#[cfg(any(test, feature = "test-support"))]
mod fake_request_error;
#[cfg(any(test, feature = "test-support"))]
mod fake_secrets_service;
#[cfg(test)]
mod fixtures;
mod invalid_release_response_error;
#[cfg(any(test, feature = "test-support"))]
mod received_release;
mod release_client;
mod release_client_config;
mod release_destination;
mod release_error;
mod release_request;
mod release_subject_error;
mod release_timeout;
mod release_timeout_error;
mod released_value;
mod runtime_service;
mod runtime_service_error;
mod service_xkey;
mod service_xkey_error;
mod tenant_account;
mod tenant_account_error;
mod unseal_error;

pub use constants::{DEFAULT_RELEASE_TIMEOUT, MAX_RELEASE_TIMEOUT, RELEASE_HOLD_BOUND};
#[cfg(any(test, feature = "test-support"))]
pub use fake_outcome::FakeOutcome;
#[cfg(any(test, feature = "test-support"))]
pub use fake_request_error::FakeRequestError;
#[cfg(any(test, feature = "test-support"))]
pub use fake_secrets_service::FakeSecretsService;
pub use invalid_release_response_error::InvalidReleaseResponseError;
#[cfg(any(test, feature = "test-support"))]
pub use received_release::ReceivedRelease;
pub use release_client::ReleaseClient;
pub use release_client_config::ReleaseClientConfig;
pub use release_destination::ReleaseDestination;
pub use release_error::ReleaseError;
pub use release_request::ReleaseRequest;
pub use release_subject_error::ReleaseSubjectError;
pub use release_timeout::ReleaseTimeout;
pub use release_timeout_error::ReleaseTimeoutError;
pub use released_value::ReleasedValue;
pub use runtime_service::RuntimeService;
pub use runtime_service_error::RuntimeServiceError;
pub use service_xkey::ServiceXkey;
pub use service_xkey_error::ServiceXkeyError;
pub use tenant_account::TenantAccount;
pub use tenant_account_error::TenantAccountError;
pub use unseal_error::UnsealError;
