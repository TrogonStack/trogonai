//! Auth-callout — NATS **`$SYS.REQ.USER.AUTH`** responder sketch (typically a separate daemon process).
//!
//! Runs **beside the NATS cluster** as a dedicated client with **System Account** (or Operator-delegated
//! callout principal) access so it can subscribe to the system auth subject and reply with minted User
//! JWTs for tenant Accounts.
//!
//! **Related:** [`docs/A2A_AUTH_CALLOUT_SKETCH.md`](../../../../docs/A2A_AUTH_CALLOUT_SKETCH.md) ·
//! [`A2A_PLAN.md`](../../../../A2A_PLAN.md) · [`A2A_TODO.md`](../../../../A2A_TODO.md)

/// NATS system subject the server publishes auth decisions to when Authorization Callout is enabled.
pub const SYS_REQ_USER_AUTH: &str = "$SYS.REQ.USER.AUTH";

/// Placeholder daemon that will subscribe to [`SYS_REQ_USER_AUTH`] and mint Account-scoped User JWTs.
///
/// Deployment model: separate process from [`crate::runtime`] gateway ingress — co-located with the
/// NATS cluster, connected via a service User on the System Account (see AUTH_CALLOUT sketch §2).
pub struct SysAuthCalloutDaemon;

impl SysAuthCalloutDaemon {
    /// Returns the system auth subject this daemon listens on.
    pub fn auth_subject(&self) -> &'static str {
        SYS_REQ_USER_AUTH
    }

    /// Stub lifecycle hook — real wiring will block on NATS `$SYS.REQ.USER.AUTH` request/reply.
    pub fn run_stub(&self) {}
}

/// Stub seam for assembling minted User JWT claim templates from verified external credentials.
///
/// Intentionally synchronous and dependency-free (no `async_trait`); future impls may wrap async
/// OIDC / mTLS verification before returning claim literals.
pub trait IssuedUserJwtBuilder {
    /// Namespaced claim key for stable caller identity (`caller_id` in AUTH_CALLOUT sketch §3).
    fn caller_id_claim(&self) -> Option<&'static str>;

    /// JWT `aud` target — tenant NATS Account public key or name.
    fn account_aud_claim(&self) -> Option<&'static str>;

    /// Placeholder — real builder will populate publish/subscribe ACL templates from policy.
    fn stub_build(&mut self) {}
}
