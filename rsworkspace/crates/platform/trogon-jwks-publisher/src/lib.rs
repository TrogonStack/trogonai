//! AAuth (draft-hardt-oauth-aauth-protocol) Agent Provider surface: mints
//! `aa-agent+jwt` identity tokens ("Obtaining an Agent Token") and publishes
//! the well-known JWKS discovery documents resources need to verify them
//! ("Metadata Documents" / RFC 8615).
//!
//! Library-only: no binary, no process lifecycle. A host service mounts
//! [`publisher::router`] into its own `axum::Router` and calls
//! [`provider::AgentProvider::mint`] wherever it issues tokens.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod provider;
pub mod publisher;
