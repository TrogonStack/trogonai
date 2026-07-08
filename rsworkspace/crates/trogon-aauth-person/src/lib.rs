//! AAuth Person Server (three-party model), per
//! draft-hardt-oauth-aauth-protocol section "Person Server"
//! (#person-server): receives an agent's token request carrying a resource
//! challenge, applies the person's policy, drives interaction/clarification
//! when needed, and mints `aa-auth+jwt`.
//!
//! # Layout
//!
//! - [`error`] -- typed errors, mapped to draft wire error codes.
//! - [`time`] -- re-exported pluggable clock ([`trogon_aauth_verify::TimeSource`]).
//! - [`agent`] -- "Agent Token Request" / "Resource Challenge Verification":
//!   verifies the inbound request before policy runs.
//! - [`subagent`] -- reads the (not-yet-modeled) `parent_agent` claim for
//!   sub-agent delegation, per "Sub-Agents".
//! - [`mint`] -- "Auth Token Structure": mints `aa-auth+jwt`.
//! - [`decision`] -- pluggable [`decision::PolicyEngine`] seam for "the
//!   person's policy" referenced throughout "PS Response".
//! - [`interaction`] -- pluggable [`interaction::InteractionChannel`] seam
//!   for "User Interaction" / "Resource-Initiated Interaction".
//! - [`pending`] -- "PS Token Endpoint" state machine: deferred responses,
//!   concurrent-request correlation, clarification chat, re-authorization.
//! - [`mission`] -- "Mission": creation/approval/log/completion state.
//! - [`store`] -- pluggable [`store::PersonStateStore`] persistence seam,
//!   plus [`store::InMemoryStore`].
//! - [`login`] -- "Third-Party Login".
//! - [`server`] -- [`server::PersonServer`], the transport-agnostic facade.
//! - [`http`] -- `axum::Router` binding.
//!
//! # Deviations from the draft
//!
//! - **No RFC 9421 HTTP Message Signature verification.** The draft's
//!   `Signature-Key` / `Signature-Input` / `Signature` / `Content-Digest`
//!   headers authenticate the outer HTTP request itself (not just the JWTs
//!   in the body). This crate's [`http`] module extracts the agent's
//!   `aa-agent+jwt` from `Signature-Key` but does not verify the
//!   request-signing envelope cryptographically. No RFC 9421 verifier exists
//!   anywhere in this workspace yet (only a NATS-adapted PoP variant in
//!   `trogon_aauth_verify::nats_pop`); the sibling `trogon-aauth-as` crate
//!   defers the identical concern to its own not-yet-built `http` module.
//!   Promoting a shared HTTP PoP verifier into `trogon-aauth-verify` is the
//!   natural fix, tracked as a promotion candidate below.
//! - **`/login` is served by this crate for demonstration/testing, but the
//!   draft's Person Server metadata document (`aauth-person.json`) defines
//!   no `login_endpoint`.** Per "Third-Party Login", `login_endpoint` is
//!   metadata published by the AGENT or the RESOURCE, not the PS -- the user
//!   is redirected *to* the PS's authentication UI, but that redirect target
//!   is conventionally the PS's own sign-in flow, not a protocol-mandated
//!   PS-side URL. [`login::parse_login_request`] and the `/login` route
//!   model the PS-side half of that flow (parsing the redirect, producing a
//!   resource token to complete it) so the crate has something concrete to
//!   test; a real deployment's actual login UI is out of scope.
//!
//! # Promotion candidates (flagged, not applied -- `trogon-identity-types`
//! is out of this crate's ownership)
//!
//! - [`trogon_identity_types::aauth::AuthClaims`] has no `dwk` field, even
//!   though "Auth Token Structure" lists `dwk` as a required claim. This
//!   crate patches the gap locally at the minting boundary
//!   ([`mint`]'s private `MintedAuthClaims`), mirroring the same patch in
//!   the sibling `trogon-aauth-as` crate. Two independent crates patching
//!   the same gap is a signal `dwk` should move onto `AuthClaims` directly.
//! - [`trogon_identity_types::aauth::AgentClaims`] has no `parent_agent`
//!   field for "Sub-Agents" delegation. This crate re-decodes the JWT
//!   payload directly ([`subagent::parent_agent_of`]) rather than widen the
//!   shared type, again mirroring `trogon-aauth-as`.
//! - A shared HTTP RFC 9421 PoP verifier (parallel to
//!   `trogon_aauth_verify::nats_pop`) belongs in `trogon-aauth-verify` so
//!   every HTTP-facing AAuth server (this crate, `trogon-aauth-as`,
//!   `a2a-gateway`) can depend on one implementation instead of each
//!   deferring it to their own `http` module.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod agent;
pub mod decision;
pub mod error;
pub mod http;
pub mod interaction;
pub mod login;
pub mod mint;
pub mod mission;
pub mod pending;
pub mod server;
pub mod store;
pub mod subagent;
pub mod time;

pub use decision::{DecisionRequest, PolicyDecision, PolicyEngine};
pub use error::PersonServerError;
pub use interaction::{InteractionChannel, InteractionNotice, NoopInteractionChannel};
pub use mission::{ApprovedMission, Mission, MissionContext, MissionId, MissionValidationError};
pub use pending::{PendingId, PendingPhase, PendingRequest};
pub use server::{PersonServer, TokenEndpointOutcome};
pub use store::{InMemoryStore, PersonStateStore};
