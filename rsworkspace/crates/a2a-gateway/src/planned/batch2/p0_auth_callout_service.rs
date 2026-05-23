//! Phase 0 — Auth callout service (**`$SYS.REQ.USER.AUTH`**) — OIDC, mTLS, API keys; mint Account User JWT.
//!
//! Compile seam for a **separate daemon process** that subscribes to the NATS system auth subject,
//! verifies external credentials, and replies with short-lived **Account-bound User JWTs** whose
//! **`aud`** is the tenant NATS Account and whose **`sub`** carries the external identity (OIDC,
//! mTLS cert subject, or transitional API-key principal).
//!
//! Credential preference order: **OIDC** (primary) → **mTLS** (service-to-service) → **API keys**
//! (transitional only).
//!
//! **Related:** [`A2A_AUTH_CALLOUT_SKETCH.md`](../../../../docs/A2A_AUTH_CALLOUT_SKETCH.md) ·
//! [`A2A_TODO.md`](../../../../A2A_TODO.md) · [`nats_auth_callout_sys`](../nats_auth_callout_sys.rs)

/// NATS system subject the server publishes auth decisions to when Authorization Callout is enabled.
pub const SYS_REQ_USER_AUTH: &str = "$SYS.REQ.USER.AUTH";

/// External credential scheme presented at NATS connect time (see AUTH_CALLOUT sketch §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityProofKind {
    /// Primary — OIDC bearer token validated via IdP introspection or JWKS.
    OidcBearer,
    /// Service-to-service — client TLS certificate thumbprint / subject mapping.
    MtlsThumbprint,
    /// Transitional — legacy API-key principal; migrate callers off this path.
    ApiKey,
}

/// Placeholder for minted User JWT claim layout (`aud` = Account, `sub` = external identity).
///
/// Real issuers populate standard claims plus `caller_id` and SpiceDB-ready `data` (sketch §3).
pub struct MintedJwtClaimsSkeleton {
    /// Stub note — production JWT binds external `sub` to Account `aud`.
    pub sub_aud_note: &'static str,
}

impl MintedJwtClaimsSkeleton {
    /// Returns the compile-time placeholder skeleton used until real claim builders land.
    pub const PLACEHOLDER: Self = Self {
        sub_aud_note: "aud=Account, sub=external identity (see A2A_AUTH_CALLOUT_SKETCH §3)",
    };
}

/// Parses inbound **`$SYS.REQ.USER.AUTH`** request payloads into verified identity context.
///
/// Intentionally synchronous (no `async_trait`); future impls may delegate to blocking OIDC /
/// mTLS verification before the daemon replies on the NATS request/reply subject.
pub trait AuthCalloutRequestParser {
    /// Detects which credential scheme the client presented.
    fn identity_proof_kind(&self, wire: &[u8]) -> IdentityProofKind;

    /// Target NATS Account public key or name the client is attempting to join.
    fn target_account(&self, wire: &[u8]) -> Option<&str>;

    /// External identity segment for JWT `sub` after verification succeeds.
    fn external_sub(&self, wire: &[u8]) -> Option<&str>;
}

/// Mints **Account-bound User JWTs** after external credential verification.
///
/// On success the returned JWT carries **`aud`** = tenant Account and **`sub`** = external
/// identity; NATS ACL templates derive from the same mint (sketch §2 reply, §3 claims).
pub trait AccountBoundUserJwtIssuer {
    /// Stub — returns a compile-time placeholder until Account signing keys are wired.
    fn issue_placeholder(&self) -> &'static str;

    /// Claim skeleton describing the minted JWT shape for downstream gateway / DLQ wiring.
    fn claims_skeleton(&self) -> MintedJwtClaimsSkeleton {
        MintedJwtClaimsSkeleton::PLACEHOLDER
    }
}

/// Phase 0 auth callout daemon — listens on [`SYS_REQ_USER_AUTH`] and replies with minted User JWTs.
///
/// Deployment: separate process beside the NATS cluster with System Account (or Operator-delegated
/// callout principal) subscription rights (AUTH_CALLOUT sketch §2).
pub struct AuthCalloutServiceDaemon<P, I> {
    parser: P,
    issuer: I,
}

impl<P, I> AuthCalloutServiceDaemon<P, I>
where
    P: AuthCalloutRequestParser,
    I: AccountBoundUserJwtIssuer,
{
    /// Returns the system auth subject this daemon listens on.
    pub fn auth_subject(&self) -> &'static str {
        SYS_REQ_USER_AUTH
    }

    /// Stub request handler — real wiring will parse, verify, mint, and reply on NATS.
    pub fn handle_auth_request_stub(&self, wire: &[u8]) -> &'static str {
        let _kind = self.parser.identity_proof_kind(wire);
        let _account = self.parser.target_account(wire);
        let _sub = self.parser.external_sub(wire);
        self.issuer.issue_placeholder()
    }

    /// Stub lifecycle hook — future impl blocks on `$SYS.REQ.USER.AUTH` request/reply loop.
    pub fn run_stub(&self) {}
}

/// No-op parser for compile-only scaffolding.
pub struct StubAuthCalloutRequestParser;

impl AuthCalloutRequestParser for StubAuthCalloutRequestParser {
    fn identity_proof_kind(&self, _wire: &[u8]) -> IdentityProofKind {
        IdentityProofKind::OidcBearer
    }

    fn target_account(&self, _wire: &[u8]) -> Option<&str> {
        None
    }

    fn external_sub(&self, _wire: &[u8]) -> Option<&str> {
        None
    }
}

/// No-op issuer for compile-only scaffolding.
pub struct StubAccountBoundUserJwtIssuer;

impl AccountBoundUserJwtIssuer for StubAccountBoundUserJwtIssuer {
    fn issue_placeholder(&self) -> &'static str {
        "stub-account-bound-user-jwt"
    }
}
