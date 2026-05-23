//! §7 roadmap — `a2a-bridge` perimeter identity coupled to NATS auth-callout minted User JWTs.
//!
//! Sketches the HTTPS bridge perimeter: external credentials terminate at the bridge, the auth
//! callout mints Account-scoped User JWTs, and downstream NATS traffic carries stable caller
//! attribution. See:
//!
//! - [`A2A_AUTH_CALLOUT_SKETCH.md`](../../../../docs/A2A_AUTH_CALLOUT_SKETCH.md) — minted User JWT contract
//! - [`A2A_BRIDGE_SKETCH.md`](../../../../docs/A2A_BRIDGE_SKETCH.md) — `a2a-bridge` HTTPS sidecar placement

/// Perimeter context after HTTPS ingress auth and auth-callout minting for a bridge session.
///
/// Placeholder fields until the bridge wires OIDC/mTLS/API-key resolution and re-mint.
pub struct AuthenticatedBridgeContext {
    /// External identity (`sub`) from OIDC, mTLS cert subject, or transitional API-key principal.
    pub external_sub: Option<String>,
    /// Tenant NATS Account public key or name (`aud` on the minted User JWT).
    pub mapped_nats_aud: Option<String>,
    /// Org-standard SpiceDB principal reference from JWT `data` — opaque to NATS ACL templates.
    pub spicedb_principal_ref: Option<String>,
}

/// Read-only view of standard claims on a minted NATS User JWT.
pub trait MintedUserClaimsView {
    /// External identity (`sub`) as issued by the IdP or mTLS/API-key layer.
    fn sub(&self) -> Option<&str>;

    /// NATS Account public key or name (`aud` = tenant boundary).
    fn aud(&self) -> Option<&str>;
}

impl MintedUserClaimsView for AuthenticatedBridgeContext {
    fn sub(&self) -> Option<&str> {
        self.external_sub.as_deref()
    }

    fn aud(&self) -> Option<&str> {
        self.mapped_nats_aud.as_deref()
    }
}
