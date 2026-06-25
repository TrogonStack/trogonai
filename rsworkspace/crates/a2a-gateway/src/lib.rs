//! A2A gateway service.
//!
//! Initial scaffold: ships the caller-identity foundation so subsequent
//! slices (config, runtime, audit, policy) can wire JWT-derived `caller_id`
//! into ingress spans and audit envelopes without reshaping the public API.
//!
//! Modules:
//! - [`caller_jwt_header`] — re-exports the wire-level header constants from
//!   `a2a-auth-callout` so callers only depend on this crate.
//! - [`jwt_caller_identity`] — resolves a verified caller identity from a
//!   minted NATS User JWT carried on the inbound message, with a
//!   labs-only header-trust fallback gated behind an env flag.

#![allow(clippy::module_name_repetitions)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod caller_jwt_header;
pub mod jwt_caller_identity;
