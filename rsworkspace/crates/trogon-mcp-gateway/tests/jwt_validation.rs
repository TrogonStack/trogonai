//! JWT ingress validation integration tests (Phase 1+, Wire-Format Pin 1).
//!
//! Scenario: the gateway validates the caller JWT before policy evaluation. The NATS
//! `Authorization` header (Pin 1) is the source of truth; client-supplied identity headers
//! and payload tenant hints are dropped or ignored when they disagree with validated claims.
//!
//! Cross-references:
//! - `docs/identity/reference-jwt.md` — signature, time, audience, issuer, KID rotation
//! - `docs/identity/reference-nats-headers.md` — ingress hardening, header authority
//! - `docs/identity/reference-error-codes.md` — `-32106 auth_expired`, `-32109 audience_mismatch`
//! - `docs/adr/0002-identity-layers.md` — identity layer ordering (JWT before policy)
//! - `reference-nats-headers.md` (JWT on NATS message headers)
//!
//! Harness pattern: live NATS broker, `mcp_nats::Config`, `McpPrefix`, `trogon_nats::NatsAuth`,
//! `GatewaySettings` with `JwtValidator` in `Require` mode, gateway ingress request/reply
//! (see `e2e_nats_forward.rs`, `initialize_handshake.rs`).
//!
//! Once JWT validation lands per `reference-jwt.md`, remove `#[ignore]`, implement Arrange /
//! Act / Assert, and verify JSON-RPC deny envelopes (`error.code`, stable `error.data.reason`
//! and audience fields) before any SpiceDB or CEL evaluation runs.

#![allow(unused_imports)]

use trogon_mcp_gateway::rpc_codes;

mod signature {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn unknown_signing_key_returns_auth_expired_bad_signature() {
        // Arrange: trust bundle with kid=B only; token signed with kid=A secret not in bundle.
        // Act: tools/list via `{prefix}.gateway.request.{server_id}.tools.list` with NATS Authorization JWT.
        // Assert: error.code == AUTH_EXPIRED (-32106) or INVALID_TOKEN per spec; data.reason == "bad_signature".
        unimplemented!(
            "verify error.code in ({}, {}) and data.reason == bad_signature",
            rpc_codes::AUTH_EXPIRED,
            rpc_codes::INVALID_TOKEN
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn tampered_payload_fails_signature_verification() {
        // Arrange: valid JWT then flip one base64 payload character before ingress.
        // Assert: auth deny before policy; data.reason == "bad_signature".
        unimplemented!("reference-jwt.md signature validation path");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn wrong_hs256_secret_not_in_trust_bundle_denies() {
        // Arrange: gateway trust bundle secret S1; token minted with unrelated secret S2.
        // Assert: deny envelope; no policy evaluator or SpiceDB RPC observed.
        unimplemented!("trust bundle mismatch per reference-jwt.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn deny_occurs_before_policy_evaluation() {
        // Arrange: policy would allow if identity were accepted; JWT signature invalid.
        // Assert: no `mcp.audit.allow.*` or SpiceDB check subject traffic for this request id.
        unimplemented!("ADR 0002 identity-before-policy ordering");
    }
}

mod time {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn exp_in_past_returns_auth_expired_with_reason_expired() {
        // Arrange: JWT with exp clearly in the past (outside leeway).
        // Act: ingress request with Authorization header.
        // Assert: error.code == AUTH_EXPIRED; data.reason == "expired".
        unimplemented!("verify error.code == {} and data.reason == expired", rpc_codes::AUTH_EXPIRED);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn nbf_in_future_returns_auth_expired_with_reason_not_yet_valid() {
        // Arrange: JWT with nbf in the future (outside leeway).
        // Assert: error.code == AUTH_EXPIRED; data.reason == "not_yet_valid".
        unimplemented!("verify data.reason == not_yet_valid per reference-jwt.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn exp_within_configured_leeway_still_accepts() {
        // Arrange: exp = now - (leeway - 1s); gateway leeway_secs from JwtIngressConfig.
        // Assert: request proceeds past JWT gate (policy may still deny).
        unimplemented!("reference-jwt.md leeway boundary");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn mid_session_expired_token_at_post_auth_gate() {
        // Arrange: first request with valid JWT; second with same session but expired exp.
        // Assert: second response AUTH_EXPIRED with data.reason == "expired".
        unimplemented!("failure-mode-matrix row 13 JWT path");
    }
}

mod audience {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn aud_claim_mismatch_returns_audience_mismatch_32109() {
        // Arrange: JwtIngressConfig.audience = expected_aud; token aud = actual_aud (different).
        // Act: ingress tools/list in enforce / require mode.
        // Assert: error.code == AUDIENCE_MISMATCH.
        unimplemented!("verify error.code == {}", rpc_codes::AUDIENCE_MISMATCH);
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn data_includes_expected_aud_and_actual_aud() {
        // Assert: data.expected_aud == gateway configured audience;
        //         data.actual_aud == token aud claim (string or array per spec).
        unimplemented!("reference-error-codes.md audience_mismatch stable data shape");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn audience_mismatch_denies_before_policy_eval() {
        // Arrange: permissive policy bundle that would allow on tenant match.
        // Assert: no policy deny/allow audit for mismatched aud request.
        unimplemented!("ADR 0002 JWT gate before CEL/SpiceDB");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn trace_id_present_on_audience_mismatch_envelope() {
        // Arrange: ingress with traceparent header.
        // Assert: error.data.trace_id matches inbound W3C trace-id.
        unimplemented!("reference-error-codes.md §2 trace_id correlation");
    }
}

mod issuer {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn iss_not_in_allowed_issuer_set_denies() {
        // Arrange: JwtIngressConfig.issuers = {trusted-a}; token iss = untrusted-b.
        // Assert: auth deny before policy; code AUTH_EXPIRED or INVALID_TOKEN per reference-jwt.md.
        unimplemented!(
            "issuer allow-list deny before policy (codes {}, {})",
            rpc_codes::AUTH_EXPIRED,
            rpc_codes::INVALID_TOKEN
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn missing_iss_claim_rejects_in_require_mode() {
        // Arrange: JWT without iss; gateway jwt mode Require.
        // Assert: ingress JSON-RPC error; no SpiceDB traffic.
        unimplemented!("reference-jwt.md required claims");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn legacy_tenant_header_ignored_when_issuer_invalid() {
        // Arrange: invalid iss but client supplies legacy tenant header on NATS message.
        // Assert: deny; tenant must not be taken from dropped/stripped header path.
        unimplemented!("reference-nats-headers.md ingress hardening");
    }
}

mod kid_rotation {
    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn token_signed_with_rotated_out_kid_a_fails() {
        // Arrange: trust bundle initially {kid=A}; rotate A out, retain kid=B.
        // Act: request with JWT header kid=A signed with former A key.
        // Assert: auth deny (bad_signature or unknown kid).
        unimplemented!("reference-jwt.md KID rotation — stale kid=A");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn token_signed_with_current_kid_b_succeeds_after_rotation() {
        // Arrange: same rotation; mint fresh token with kid=B.
        // Assert: JWT gate passes; request reaches policy/backend stub.
        unimplemented!("reference-jwt.md KID rotation — current kid=B");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn overlapping_rotation_window_accepts_both_kids() {
        // Arrange: trust bundle contains A and B during overlap window.
        // Assert: tokens with kid=A and kid=B both pass signature validation.
        unimplemented!("reference-jwt.md rotation overlap window");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn jwks_refresh_picks_up_new_kid_without_restart() {
        // Arrange: JwtIngressConfig.jwks_uri with rotating JWKS document.
        // Act: publish new key kid=B; request with B-signed token.
        // Assert: succeeds without gateway process restart.
        unimplemented!("JWKS-driven kid rotation per reference-jwt.md");
    }
}

mod transport {
    use super::rpc_codes;

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn missing_authorization_header_denies_before_policy() {
        // Arrange: JwtMode::Require; ingress request with empty HeaderMap (no Authorization).
        // Assert: error.code == AUTH_REQUIRED or AUTH_EXPIRED per spec; no policy eval.
        unimplemented!(
            "missing JWT on NATS ingress (codes {}, {})",
            rpc_codes::AUTH_REQUIRED,
            rpc_codes::AUTH_EXPIRED
        );
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn nats_authorization_header_is_jwt_source_of_truth() {
        // Arrange: valid JWT only on NATS Authorization header (Pin 1), not in JSON-RPC params.
        // Assert: gateway resolves identity from header; backend egress mcp-caller-sub from JWT sub.
        unimplemented!("reference-nats-headers.md");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn client_supplied_mcp_tenant_header_dropped_on_ingress() {
        // Arrange: JWT tenant=acme; client sets mcp-tenant=evil on ingress headers.
        // Assert: policy/audit tenant binding uses JWT acme; forged header not authoritative.
        unimplemented!("reference-nats-headers.md §2 ingress hardening mcp-tenant");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when JWT validation per docs/identity/reference-jwt.md lands"]
    async fn jsonrpc_payload_tenant_does_not_override_validated_jwt_tenant() {
        // Arrange: JWT tenant=acme; JSON-RPC params include tenant=other.
        // Assert: gateway identity tenant remains acme from validated JWT only.
        unimplemented!("reference-nats-headers.md identity never from JSON-RPC alone");
    }
}
