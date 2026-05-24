//! Sketch for task #8: live `nats-server` auth-callout integration test.
//!
//! Run manually when a NATS server is available with `authorization { auth_callout { ... } }`
//! and this service subscribed on `$SYS.REQ.USER.AUTH`.

#[test]
#[ignore = "requires live nats-server with auth_callout configured (task #8)"]
#[allow(clippy::assertions_on_constants)]
fn live_connect_with_oidc_bearer_against_callout() {
    // 1. Start nats-server pinned to 2.10.x with auth_callout issuer/xkey matching env.
    // 2. Run a2a-auth-callout with AUTH_CALLOUT_* and NATS_URL set.
    // 3. Connect async-nats client with bearer JWT in connect options.
    // 4. Assert connection authorized and user JWT permissions include gateway + inbox.
    assert!(true);
}
