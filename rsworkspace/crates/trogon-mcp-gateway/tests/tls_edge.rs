//! TLS and mTLS edge integration tests (scaffold).
//!
//! Scenario: the MCP gateway connects to NATS over TLS and may require client mTLS at
//! ingress via NATS auth callout. Production config rejects plaintext NATS URLs; server
//! cert verification uses a configured CA bundle; ingress mTLS enforces client certs and
//! SAN patterns; server cert and trust-bundle rotation apply on reload without process
//! restart; SPIFFE SVID mode validates SPIFFE IDs encoded in SAN URI; only TLS 1.3 is
//! permitted.
//!
//! Cross-references:
//! - TLS, mTLS, trust bundles
//! - SPIFFE SVID specification (SAN URI `spiffe://` workload identity)
//! - Previously merged spire-wiring branch (SPIFFE / SPIRE workload integration)
//! - `docs/adr/0006-mesh-token-signing-keys.md` (trust-bundle PEM rotation on reload)
//!
//! Harness pattern: `mcp_nats::Config`, `McpPrefix`, `trogon_nats::{NatsConfig, NatsAuth}`,
//! `GatewaySettings`, `AllowAllPermissionChecker`, `trogon_mcp_gateway::run`
//! (see `tests/e2e_nats_forward.rs`). TLS fixtures: temp PEM directories, NATS with
//! TLS/mTLS listeners, auth callout service or stub returning typed rejection codes.
//!
//! Verification contract (implement when un-ignored):
//! 1. Plaintext `nats://` in production config -> startup fails with `tls_required`.
//! 2. Self-signed server cert not in CA bundle -> NATS connect fails.
//! 3. Ingress without client cert -> auth callout rejects with `mtls_required`.
//! 4. Client cert SAN mismatch -> rejected with `mtls_san_mismatch`.
//! 5. Server cert rotation on disk -> picked up on next reload without restart.
//! 6. `spiffe_enabled=true` -> SPIFFE ID in SAN URI validated.
//! 7. CA removed from trust bundle -> clients chained to that CA fail on next handshake.
//! 8. TLS 1.2 handshake -> rejected with `tls_version_unsupported` (TLS 1.3 only).

#![allow(unused_imports)]

/// Shared harness notes and error-code pins (see `tests/e2e_nats_forward.rs`).
mod harness {
    use std::time::Duration;

    #[allow(dead_code)]
    pub const TLS_EDGE_IGNORE: &str =
        "scaffold; implement when TLS edge hardening per Block G lands";

    #[allow(dead_code)]
    pub const ERR_TLS_REQUIRED: &str = "tls_required";
    #[allow(dead_code)]
    pub const ERR_MTLS_REQUIRED: &str = "mtls_required";
    #[allow(dead_code)]
    pub const ERR_MTLS_SAN_MISMATCH: &str = "mtls_san_mismatch";
    #[allow(dead_code)]
    pub const ERR_TLS_VERSION_UNSUPPORTED: &str = "tls_version_unsupported";

    #[allow(dead_code)]
    pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    #[allow(dead_code)]
    pub const TLS_MIN_VERSION: &str = "1.3";

    // Types to wire when implementing: `trogon_nats::NatsConfig`, `trogon_nats::NatsAuth`,
    // `mcp_nats::Config`, `mcp_nats::McpPrefix`, `trogon_mcp_gateway::gateway::GatewaySettings`,
    // `trogon_mcp_gateway::authz::AllowAllPermissionChecker`, `trogon_mcp_gateway::run`.
}

mod plaintext {
    //! Production config must not allow plaintext NATS URLs.

    use super::harness::ERR_TLS_REQUIRED;

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn production_plaintext_nats_url_fails_startup_with_tls_required() {
        // Arrange: production profile YAML with `nats://127.0.0.1:4222` (no tls://).
        // Act: start gateway process or `trogon_mcp_gateway::run` with parsed config.
        // Assert: startup error code == `tls_required`; process exits non-zero.
        let _ = ERR_TLS_REQUIRED;
        unimplemented!("plaintext NATS URL rejected at startup in production");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn tls_required_error_is_surfaced_before_nats_subscribe() {
        // Arrange: invalid plaintext URL in production config.
        // Act: attempt gateway bootstrap.
        // Assert: no queue-group subscription established; error logged with tls_required.
        unimplemented!("fail fast before NATS consumer bind");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn dev_profile_may_allow_plaintext_when_tls_required_false() {
        // Arrange: dev/staging profile with explicit `tls_required: false`.
        // Act: start gateway with `nats://` broker (local test harness only).
        // Assert: gateway starts; distinct from production default.
        unimplemented!("dev override for local plaintext NATS per Block G");
    }
}

mod ca_verify {
    //! Gateway verifies NATS server cert against configured CA bundle.

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn self_signed_server_cert_not_in_ca_bundle_connect_fails() {
        // Arrange: NATS with self-signed server cert; gateway CA bundle excludes that issuer.
        // Act: `trogon_nats::connect` via gateway bootstrap.
        // Assert: connect error; no MCP subscriptions.
        unimplemented!("untrusted server cert rejected against CA bundle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn valid_server_chain_against_ca_bundle_connects() {
        // Arrange: NATS server cert signed by CA present in gateway `ca_bundle_path`.
        // Act: gateway startup and NATS client connect.
        // Assert: connection established; gateway.run proceeds past NATS bind.
        unimplemented!("trusted server chain accepted");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn expired_server_cert_in_chain_fails_verify() {
        // Arrange: server cert past NotAfter; CA otherwise trusted.
        // Act: TLS handshake to NATS.
        // Assert: connect fails before auth callout.
        unimplemented!("expired server cert fails PKIX validation");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn wrong_sni_hostname_fails_server_cert_verify() {
        // Arrange: valid CA chain but cert CN/SAN does not match configured NATS host.
        // Act: connect with expected hostname mismatch.
        // Assert: TLS verify failure; no gateway ingress.
        unimplemented!("hostname verification against server cert SAN/CN");
    }
}

mod mtls_required {
    //! Ingress mTLS: NATS auth callout requires a client certificate.

    use super::harness::ERR_MTLS_REQUIRED;

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn ingress_without_client_cert_rejected_with_mtls_required() {
        // Arrange: NATS ingress with mTLS required; client presents server-only TLS (no cert).
        // Act: CONNECT through auth callout path.
        // Assert: callout returns `mtls_required`; connection closed.
        let _ = ERR_MTLS_REQUIRED;
        unimplemented!("missing client cert rejected at ingress");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn ingress_with_valid_client_cert_passes_callout() {
        // Arrange: client cert signed by ingress CA; SAN matches policy.
        // Act: CONNECT; await scoped user JWT from callout.
        // Assert: NATS user JWT issued; `mcp.gateway.request.>` ACL present.
        unimplemented!("valid mTLS client cert accepted at callout");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn mtls_required_surfaces_in_auth_callout_response_body() {
        // Arrange: callout observability or structured error envelope enabled.
        // Act: CONNECT without client cert.
        // Assert: rejection payload includes code `mtls_required`.
        unimplemented!("typed callout error code for missing client cert");
    }
}

mod san {
    //! Client cert SAN must match configured workload pattern.

    use super::harness::ERR_MTLS_SAN_MISMATCH;

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn client_cert_san_mismatch_rejected_with_mtls_san_mismatch() {
        // Arrange: ingress policy expects `*.agents.example.com`; client cert SAN is unrelated.
        // Act: CONNECT with otherwise valid mTLS cert.
        // Assert: callout rejects with `mtls_san_mismatch`.
        let _ = ERR_MTLS_SAN_MISMATCH;
        unimplemented!("SAN pattern mismatch rejected at ingress");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn client_cert_san_matching_pattern_accepted() {
        // Arrange: client cert DNS SAN matches configured allow pattern.
        // Act: CONNECT through callout.
        // Assert: JWT issued; tenant/workload claims populated from cert mapping.
        unimplemented!("matching SAN grants ingress identity");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn multiple_sans_one_match_is_sufficient() {
        // Arrange: cert carries several SAN entries; one matches policy.
        // Act: CONNECT.
        // Assert: accepted when any SAN satisfies pattern.
        unimplemented!("OR semantics across client cert SAN entries");
    }
}

mod rotation {
    //! Server certificate rotation without gateway process restart.

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn server_cert_rotation_picked_up_on_sighup_reload() {
        // Arrange: gateway running with TLS to NATS; replace server cert PEM on disk.
        // Act: SIGHUP or config reload hook re-reads cert paths.
        // Assert: subsequent handshakes present new server cert; process PID unchanged.
        unimplemented!("server cert reload without restart per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn in_flight_requests_complete_before_new_server_cert_required() {
        // Arrange: slow in-flight MCP request during cert rotation.
        // Act: reload while request active.
        // Assert: in-flight completes; new connections use rotated cert.
        unimplemented!("graceful in-flight completion across cert rotation");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn invalid_replacement_server_cert_rejects_reload() {
        // Arrange: write corrupt or expired PEM to server cert path.
        // Act: SIGHUP reload.
        // Assert: prior cert remains active; reload error metric/log emitted.
        unimplemented!("reject bad server cert on reload, keep prior material");
    }
}

mod spiffe {
    //! SPIFFE SVID integration when `spiffe_enabled=true`.

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn spiffe_enabled_validates_san_uri_spiffe_id() {
        // Arrange: `spiffe_enabled=true`; client cert SAN URI `spiffe://acme.local/ns/prod/sa/agent`.
        // Act: ingress auth callout maps cert to workload identity.
        // Assert: SPIFFE ID accepted; JWT `wkl` claim matches SPIFFE ID.
        unimplemented!("SPIFFE ID from SAN URI per SPIFFE SVID spec and spire-wiring");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn spiffe_id_not_in_trust_domain_rejected() {
        // Arrange: cert SAN URI outside configured trust domain allowlist.
        // Act: CONNECT.
        // Assert: rejection before JWT mint (SAN/SPIFFE policy failure).
        unimplemented!("SPIFFE trust domain enforcement");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn spiffe_disabled_falls_back_to_dns_san_rules() {
        // Arrange: `spiffe_enabled=false`; DNS SAN only on client cert.
        // Act: CONNECT with valid DNS SAN pattern.
        // Assert: accepted without requiring `spiffe://` URI in cert.
        unimplemented!("non-SPIFFE SAN path when spiffe_enabled=false");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn spiffe_enabled_rejects_cert_without_uri_san() {
        // Arrange: `spiffe_enabled=true`; client cert has DNS SAN only (no URI SAN).
        // Act: CONNECT.
        // Assert: rejected; SPIFFE ID not derivable from cert.
        unimplemented!("URI SAN required when SPIFFE mode enabled");
    }
}

mod trust_bundle {
    //! Trust bundle rotation invalidates clients chained to removed CAs.

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn removed_ca_invalidates_client_chain_on_next_handshake() {
        // Arrange: client cert issued by CA-A; bundle initially includes CA-A.
        // Act: remove CA-A PEM from bundle path; SIGHUP reload; new CONNECT/handshake.
        // Assert: client rejected immediately on next handshake (no restart).
        unimplemented!("trust bundle shrink rejects previously valid client chains");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn added_ca_accepts_new_issuer_on_reload() {
        // Arrange: client cert signed by CA-B not yet in bundle.
        // Act: append CA-B to bundle; reload.
        // Assert: subsequent handshakes accept client chained to CA-B.
        unimplemented!("trust bundle growth enables new issuers on reload");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn trust_bundle_reload_does_not_require_process_restart() {
        // Arrange: gateway PID recorded; bundle path updated on disk.
        // Act: SIGHUP reload trust bundle only.
        // Assert: same PID; JWT validation uses new bundle for new connections.
        unimplemented!("trust bundle hot reload per ADR-0006 and Block G");
    }
}

mod tls_version {
    //! TLS 1.3 only; older protocol versions rejected.

    use super::harness::{ERR_TLS_VERSION_UNSUPPORTED, TLS_MIN_VERSION};

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn tls_1_3_client_handshake_succeeds() {
        // Arrange: NATS/gateway TLS stack min version 1.3; client offers TLS 1.3 only.
        // Act: connect.
        // Assert: handshake completes; MCP path operational.
        let _ = TLS_MIN_VERSION;
        unimplemented!("TLS 1.3 client accepted");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn tls_1_2_handshake_rejected_with_tls_version_unsupported() {
        // Arrange: client forced to TLS 1.2 cipher suite offer only.
        // Act: connect to gateway or NATS TLS listener.
        // Assert: handshake fails with `tls_version_unsupported`.
        let _ = ERR_TLS_VERSION_UNSUPPORTED;
        unimplemented!("TLS 1.2 rejected when min version is 1.3");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn tls_1_0_and_1_1_offers_rejected() {
        // Arrange: legacy client profile (TLS 1.0/1.1).
        // Act: connect attempt.
        // Assert: same `tls_version_unsupported` path as 1.2.
        unimplemented!("legacy TLS versions rejected");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when TLS edge hardening per Block G lands"]
    async fn tls_version_unsupported_is_logged_on_handshake_failure() {
        // Arrange: structured gateway/NATS TLS logging enabled.
        // Act: TLS 1.2 connect attempt.
        // Assert: log/metric includes `tls_version_unsupported` reason.
        unimplemented!("observable tls_version_unsupported on downgrade attempts");
    }
}
