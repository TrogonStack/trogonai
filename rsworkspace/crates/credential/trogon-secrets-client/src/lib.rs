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
//! [`CachedReleaseClient`] holds each release it fetched until
//! [`RELEASE_CACHE_JITTER`] or less before its hold ends, for one tenant,
//! credential, destination, and location, and at most
//! [`MAX_CACHED_RELEASES`] at once (ADR#0049, ADR#0023 Decision 6). A held
//! copy also expires on the clock's monotonic side, never more than
//! [`RELEASE_HOLD_BOUND`] after it was held, so a wall clock stepped back
//! cannot stretch its hold. Its own
//! reader of [`INVALIDATION_STREAM`] drops every held copy of a credential
//! named on `secrets.invalidation.v1.{tenant}.{owner_token}.{credential_id}`,
//! and every held copy at all for a notice it cannot read. A release on its
//! way when its credential is invalidated, or when the reader loses or
//! regains its place, is neither held nor returned; the client asks once more
//! and fails with [`ReleaseError::Invalidated`] if that one is overtaken too. It refuses to
//! start without that stream, or with one whose configuration could remove or
//! rewrite a notice inside the lookback ([`InvalidationStreamError`]); the
//! stream's capacity is left to its operator. Every [`INVALIDATION_POSITION_CHECK`] without a
//! notice it asks the server how far its consumer delivered, and a notice
//! delivered but still not received at the next look loses it its place.
//! While its reader has lost its place it holds
//! nothing, and it reads again every [`INVALIDATION_RETRY_DELAY`] from
//! [`INVALIDATION_LOOKBACK`] back.
//!
//! Concurrent misses for one key share one call to the secrets service. The
//! call runs apart from the caller that started it, so it lands for the rest
//! when that caller leaves; each caller waits up to its own deadline. A
//! failure reaches every caller waiting on that call and is never held, and
//! a call an invalidation overtook is neither joined, held, nor returned.
//!
//! # Telemetry
//!
//! On the `trogon-secrets-client` meter, [`CachedReleaseClient`] counts
//! `secrets.client.invalidation.position_losses`, under
//! `secrets.client.invalidation.loss_reason`, each time its reader loses its
//! place, and `secrets.client.release_cache.capacity_skips` for each release
//! it serves without holding because the cache is full. Each loss of place is
//! also a warning event, and reading again an info event; a full cache warns
//! at most once a minute, with how many releases it turned away since.
//!
//! # Features
//!
//! - `test-support`: adds `FakeSecretsService`, a scripted stand-in for the
//!   secrets service that seals with its own xkey and records what it was
//!   asked, for crates that test against the release call. Off by default;
//!   enable it only from `[dev-dependencies]`.

mod cached_release;
mod cached_release_client;
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
mod invalidation_consumer;
mod invalidation_loss;
mod invalidation_notice;
mod invalidation_reader;
mod invalidation_start_error;
#[cfg(test)]
mod invalidation_step;
mod invalidation_stream;
mod invalidation_stream_error;
mod invalidation_stream_source;
#[cfg(any(test, feature = "test-support"))]
mod received_release;
mod release_cache;
mod release_cache_key;
mod release_cache_mode;
mod release_client;
mod release_client_config;
mod release_destination;
mod release_error;
mod release_flight;
mod release_flight_landing;
mod release_flights;
mod release_hold;
mod release_invalidation;
mod release_request;
mod release_subject_error;
mod release_timeout;
mod release_timeout_error;
mod released_value;
mod runtime_service;
mod runtime_service_error;
#[cfg(test)]
mod scripted_invalidations;
#[cfg(test)]
mod scripted_notice;
mod service_xkey;
mod service_xkey_error;
#[cfg(test)]
mod stepped_clock;
mod telemetry;
mod tenant_account;
mod tenant_account_error;
mod unseal_error;

pub(crate) use cached_release::CachedRelease;
pub use cached_release_client::CachedReleaseClient;
pub use constants::{
    DEFAULT_RELEASE_TIMEOUT, INVALIDATION_LOOKBACK, INVALIDATION_POSITION_CHECK, INVALIDATION_RETRY_DELAY,
    INVALIDATION_STREAM, MAX_CACHED_RELEASES, MAX_RELEASE_TIMEOUT, RELEASE_CACHE_JITTER, RELEASE_HOLD_BOUND,
};
#[cfg(any(test, feature = "test-support"))]
pub use fake_outcome::FakeOutcome;
#[cfg(any(test, feature = "test-support"))]
pub use fake_request_error::FakeRequestError;
#[cfg(any(test, feature = "test-support"))]
pub use fake_secrets_service::FakeSecretsService;
pub use invalid_release_response_error::InvalidReleaseResponseError;
pub use invalidation_consumer::InvalidationConsumer;
pub(crate) use invalidation_loss::InvalidationLoss;
pub use invalidation_notice::InvalidationNotice;
pub(crate) use invalidation_reader::InvalidationReader;
pub use invalidation_start_error::InvalidationStartError;
#[cfg(test)]
pub(crate) use invalidation_step::InvalidationStep;
pub use invalidation_stream::InvalidationStream;
pub use invalidation_stream_error::InvalidationStreamError;
pub use invalidation_stream_source::InvalidationStreamSource;
#[cfg(any(test, feature = "test-support"))]
pub use received_release::ReceivedRelease;
pub(crate) use release_cache::ReleaseCache;
pub(crate) use release_cache_key::ReleaseCacheKey;
pub use release_cache_mode::ReleaseCacheMode;
pub use release_client::ReleaseClient;
pub use release_client_config::ReleaseClientConfig;
pub use release_destination::ReleaseDestination;
pub use release_error::ReleaseError;
pub(crate) use release_flight::ReleaseFlight;
pub(crate) use release_flight_landing::ReleaseFlightLanding;
pub(crate) use release_flights::ReleaseFlights;
pub(crate) use release_hold::ReleaseHold;
pub(crate) use release_invalidation::ReleaseInvalidation;
pub use release_request::ReleaseRequest;
pub use release_subject_error::ReleaseSubjectError;
pub use release_timeout::ReleaseTimeout;
pub use release_timeout_error::ReleaseTimeoutError;
pub use released_value::ReleasedValue;
pub use runtime_service::RuntimeService;
pub use runtime_service_error::RuntimeServiceError;
#[cfg(test)]
pub(crate) use scripted_invalidations::ScriptedInvalidations;
#[cfg(test)]
pub(crate) use scripted_notice::ScriptedNotice;
pub use service_xkey::ServiceXkey;
pub use service_xkey_error::ServiceXkeyError;
#[cfg(test)]
pub(crate) use stepped_clock::SteppedClock;
pub use tenant_account::TenantAccount;
pub use tenant_account_error::TenantAccountError;
pub use unseal_error::UnsealError;
