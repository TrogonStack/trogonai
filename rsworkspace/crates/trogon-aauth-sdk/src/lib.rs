//! Agent-side SDK for AAuth.
//!
//! Provides keypair generation, bootstrap helpers, request signing for both
//! HTTP (RFC 9421) and NATS (custom envelope mirroring RFC 9421), and the
//! 401 → exchange → retry loop that turns a resource challenge into an
//! `aa-auth+jwt`.
//!
//! Two clients are offered for talking to a Person Server:
//!  * [`PersonHttpClient`] — JSON over HTTPS to `/aauth/agent` and `/aauth/token`.
//!  * [`PersonNatsClient`] — NATS req/rep on `aauth.{ps_id}.bootstrap` and
//!    `aauth.{ps_id}.token`.
//!
//! The transport-specific signers ([`HttpRequestSigner`], [`NatsRequestSigner`])
//! attach the agent identity token (`aa-agent+jwt`) and proof-of-possession
//! headers to outbound requests so a resource can verify the agent's binding.

pub mod bootstrap;
pub mod client;
pub mod error;
pub mod keypair;
pub mod retry;
pub mod sign;

pub use bootstrap::{BootstrapRequest, BootstrapResponse, TokenRequest, TokenResponse};
pub use client::{PersonHttpClient, PersonNatsClient};
pub use error::SdkError;
pub use keypair::AgentKeypair;
pub use retry::{ChallengeOutcome, parse_challenge_headers};
pub use sign::{HttpRequestSigner, NatsRequestSigner, SignedNatsRequest};
