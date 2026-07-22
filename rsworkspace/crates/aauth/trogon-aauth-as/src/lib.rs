//! AAuth Access Server (`trogon-aauth-as`): the four-party organization-side
//! issuer defined by draft-hardt-oauth-aauth-protocol's "Access Server
//! Federation" (#access-server-federation). Receives a PS-to-AS token
//! request, applies organization policy, optionally demands identity
//! claims, and issues `aa-auth+jwt` for organization resources.
//!
//! # Draft section to module map
//!
//! | Draft section | Module |
//! |---|---|
//! | "Federated Access (Four-Party)" (#overview, four-party flow) | `server` (`FederationMode`, orchestration) |
//! | "AS Token Endpoint" / "PS-to-AS Token Request" / "AS Response" (#as-token-endpoint) | `server` (`AccessServer::evaluate`, `AsOutcome`), `http` |
//! | "Auth Token Delivery" (#as-token-endpoint) | `mint` |
//! | "Claims Required" (#requirement-claims) | `pending`, `policy::Decision::RequireClaims` |
//! | "PS-AS Federation" / "PS-AS Trust Establishment" (#ps-as-federation) | `trust` |
//! | "AS Decision Logic (Non-Normative)" (#as-decision-logic) | `policy` |
//! | "Organization Visibility" / "PS-AS Collapse" (#ps-as-collapse) | `server::FederationMode::Collapsed` |
//! | "Auth Token" / "Auth Token Structure" (#auth-tokens) | `mint` |
//! | "Auth Token Verification" / "Upstream Token Verification" (#upstream-token-verification) | `verify` |
//! | "Delegation Chain" (#delegation-chain), "Sub-Agents" (#sub-agents), "Call Chaining" (#call-chaining) | `mint::nest_act`, `subagent`, `server::act_for` |
//!
//! # Local types flagged for promotion to `trogon-identity-types`
//!
//! - [`trogon_identity_types::aauth::AuthClaims`] has no `dwk` field, even
//!   though "Auth Token Structure" lists `dwk` as a required auth-token
//!   claim (`aauth-access.json` for AS-issued tokens, `aauth-person.json`
//!   for PS-issued ones). Worked around locally in `mint` via a
//!   `#[serde(flatten)]` wrapper that adds `dwk` at the serialization
//!   boundary; `trogon-aauth-verify::TokenVerifier::verify_auth` has the
//!   same gap on the read side (does not check `dwk` on auth tokens the way
//!   it does for agent/resource tokens).
//! - [`trogon_identity_types::aauth::AuthClaims::sub`] is a required
//!   `String`, but the draft only requires "at least one of `sub` or
//!   `scope`". This crate always populates `sub` (falling back to the
//!   binding agent identifier when no directed identifier is available yet)
//!   because the current type cannot represent a `scope`-only token; see
//!   `server::subject_for`.
//! - [`trogon_identity_types::aauth::AgentClaims`] has no `parent_agent`
//!   field, even though "Agent Token Structure" defines it and "Sub-Agents"
//!   depends on it throughout. Worked around locally in `subagent` by
//!   decoding the field directly off the already-verified JWT payload.
//!
//! # Deviations
//!
//! - Only `requirement=claims` is modeled as a pause/resume state
//!   ([`pending::PendingRequestStore`]). `requirement=interaction`,
//!   `requirement=approval`, and `402 Payment Required` extend *trust
//!   itself* (#ps-as-trust-establishment) rather than feeding a single
//!   request's policy decision, and are left to a future slice / to the
//!   [`trust::TrustRegistry`] being populated out of band once those flows
//!   complete elsewhere.
//! - [`policy::OrganizationPolicy`] is synchronous. Deployments needing an
//!   external PDP call out before constructing the
//!   [`request::AsTokenContext`], keeping `AccessServer` free of I/O.

#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod error;
pub mod http;
pub mod mint;
pub mod pending;
pub mod policy;
pub mod request;
pub mod server;
pub mod subagent;
pub mod trust;
pub mod verify;

pub use error::{AccessServerError, MintError, RequestVerificationError};
pub use mint::{AuthTokenInputs, AuthTokenTtl, mint_auth_jwt, nest_act};
pub use pending::{PendingRequest, PendingRequestId, PendingRequestStore, ResumeBody};
pub use policy::{AlwaysIssuePolicy, Decision, GrantedScope, OrganizationPolicy, RequiredClaims};
pub use request::{AsTokenContext, BindingAgent};
pub use server::{AccessServer, AccessServerConfig, AsOutcome, FederationMode};
pub use trust::{PsIssuer, TrustBasis, TrustBasisRecord, TrustRegistry, TrustRegistryError, TrustedIssuer};
pub use verify::{VerifiedRequest, verify_request};

#[cfg(test)]
pub(crate) mod test_support;
