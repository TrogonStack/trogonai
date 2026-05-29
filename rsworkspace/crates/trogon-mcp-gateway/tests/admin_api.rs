//! Integration test scaffold for the operator-facing admin HTTP API (Phase 2).
//!
//! Scenario: Phase 2 ships an admin listener for bundle and config management, detailed
//! health introspection, dry-run policy inspection, JWT role gating, dedicated audit
//! envelopes, feature-flagged mount (`admin_api_enabled`), and optional mTLS on the admin port.
//!
//! Cross-references:
//! - admin API, reload, rollback, status
//! - `tests/e2e_nats_forward.rs` — harness pattern (`GatewaySettings`, `McpPrefix`,
//!   `trogon_nats::NatsAuth`, `AllowAllPermissionChecker`, `trogon_mcp_gateway::run`)
//!
//! Verification contract (implement when un-ignored):
//! 1. `POST /admin/reload` triggers config + bundle reload; responds with
//!    `{config_sha256, bundle_sha256, outcome}`.
//! 2. `POST /admin/bundle/rollback` reverts to previous bundle; responds with previous + new sha.
//! 3. `GET /admin/status` returns NATS state, SpiceDB state, in-flight requests, buffer depths.
//! 4. `GET /admin/policy/inspect?subject=...&method=...&tool=...` returns dry-run decision with
//!    `decision`, `rule_fired`, `trace_id`.
//! 5. Admin endpoints require `admin` role in JWT; absent role -> HTTP 403.
//! 6. Admin endpoints emit dedicated audit envelope `admin.{action}` separate from request audits.
//! 7. Admin endpoints are NOT mounted when `admin_api_enabled=false` (default off in production).
//! 8. When `admin_require_mtls=true`, missing client cert -> HTTP 401.

#![allow(unused_imports)]

/// Shared harness notes (see `tests/e2e_nats_forward.rs`).
mod harness {
    use std::time::Duration;

    #[allow(dead_code)]
    pub const ADMIN_IGNORE: &str =
        "scaffold; implement when admin API lands";
    #[allow(dead_code)]
    pub const ADMIN_LISTEN_ADDR: &str = "127.0.0.1:8081";
    #[allow(dead_code)]
    pub const RELOAD_PATH: &str = "/admin/reload";
    #[allow(dead_code)]
    pub const ROLLBACK_PATH: &str = "/admin/bundle/rollback";
    #[allow(dead_code)]
    pub const STATUS_PATH: &str = "/admin/status";
    #[allow(dead_code)]
    pub const POLICY_INSPECT_PATH: &str = "/admin/policy/inspect";
    #[allow(dead_code)]
    pub const NATS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
    #[allow(dead_code)]
    pub const ADMIN_ROLE: &str = "admin";
}

mod reload {
    //! `POST /admin/reload` triggers config + bundle reload.

    use super::harness::{ADMIN_LISTEN_ADDR, RELOAD_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_reload_returns_config_and_bundle_sha256_with_outcome() {
        // Arrange: gateway with admin_api_enabled; valid admin JWT; note current config/bundle hashes.
        // Act: POST http://{ADMIN_LISTEN_ADDR}{RELOAD_PATH}.
        // Assert: 200; body has config_sha256, bundle_sha256, outcome (success|failure).
        let _ = (ADMIN_LISTEN_ADDR, RELOAD_PATH);
        unimplemented!("reload response envelope per Block G");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_reload_applies_config_and_bundle_from_pinned_paths() {
        // Arrange: mutate config YAML and bundle KV revision before reload.
        // Act: POST /admin/reload.
        // Assert: gateway serves new config/bundle; returned sha256 values match post-reload state.
        unimplemented!("reload triggers config + bundle hot-swap");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_reload_in_flight_requests_retain_prior_config_until_complete() {
        // Arrange: slow in-flight MCP request; prepare config change visible only after reload.
        // Act: POST /admin/reload while request active.
        // Assert: in-flight completes under old config; subsequent requests observe new config.
        unimplemented!("reload does not disrupt in-flight request config pin");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_reload_failure_returns_outcome_failure_with_prior_sha256() {
        // Arrange: invalid bundle revision or config validation error staged.
        // Act: POST /admin/reload.
        // Assert: outcome == failure; config_sha256/bundle_sha256 reflect last good state.
        unimplemented!("failed reload reports outcome without applying bad revision");
    }
}

mod rollback {
    //! `POST /admin/bundle/rollback` reverts to the previous bundle revision.

    use super::harness::{ADMIN_LISTEN_ADDR, ROLLBACK_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_rollback_returns_previous_and_new_bundle_sha256() {
        // Arrange: gateway with at least two bundle revisions loaded; note current sha.
        // Act: POST http://{ADMIN_LISTEN_ADDR}{ROLLBACK_PATH}.
        // Assert: 200; body includes previous_sha256 and new_sha256 (reverted revision).
        let _ = (ADMIN_LISTEN_ADDR, ROLLBACK_PATH);
        unimplemented!("rollback response with previous + new bundle sha");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_rollback_restores_prior_policy_evaluation_behavior() {
        // Arrange: bundle N denies tool X; bundle N-1 allows tool X.
        // Act: POST /admin/bundle/rollback after loading bundle N.
        // Assert: dry-run or live tools/call for tool X matches bundle N-1 policy.
        unimplemented!("rollback swaps active bundle snapshot");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn post_rollback_without_prior_revision_returns_error_outcome() {
        // Arrange: gateway with only initial bundle revision (no rollback target).
        // Act: POST /admin/bundle/rollback.
        // Assert: 4xx or 200 with outcome failure; bundle sha unchanged.
        unimplemented!("rollback rejected when history empty");
    }
}

mod status {
    //! `GET /admin/status` returns detailed operational health.

    use super::harness::{ADMIN_LISTEN_ADDR, STATUS_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_status_reports_nats_and_spicedb_state() {
        // Arrange: gateway with live NATS; SpiceDB reachable or stubbed.
        // Act: GET http://{ADMIN_LISTEN_ADDR}{STATUS_PATH} with admin JWT.
        // Assert: body includes nats.state and spicedb.state (connected|degraded|down).
        let _ = (ADMIN_LISTEN_ADDR, STATUS_PATH);
        unimplemented!("status includes NATS and SpiceDB sub-objects");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_status_reports_in_flight_request_count() {
        // Arrange: hold slow backend; start multiple concurrent MCP requests.
        // Act: GET /admin/status during in-flight window.
        // Assert: in_flight_requests >= active request count.
        unimplemented!("status exposes in-flight request gauge");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_status_reports_buffer_depths_per_lane() {
        // Arrange: gateway under load or with configured buffer metrics.
        // Act: GET /admin/status.
        // Assert: buffer_depths object present with per-subject or per-lane depths.
        unimplemented!("status includes buffer depth introspection");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_status_reflects_degraded_nats_without_process_exit() {
        // Arrange: sever NATS connection while process stays alive.
        // Act: GET /admin/status.
        // Assert: nats.state != connected; HTTP 200 with degraded detail (not 503 on admin path).
        unimplemented!("admin status reports dependency degradation");
    }
}

mod policy_inspect {
    //! `GET /admin/policy/inspect` dry-runs policy without forwarding.

    use super::harness::{ADMIN_LISTEN_ADDR, POLICY_INSPECT_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_policy_inspect_returns_decision_rule_fired_and_trace_id() {
        // Arrange: bundle with known allow/deny rule for subject/method/tool triple.
        // Act: GET /admin/policy/inspect?subject=...&method=tools/call&tool=fixture.
        // Assert: body has decision, rule_fired, trace_id.
        let _ = (ADMIN_LISTEN_ADDR, POLICY_INSPECT_PATH);
        unimplemented!("policy inspect dry-run response envelope");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_policy_inspect_does_not_publish_request_audit_envelope() {
        // Arrange: JetStream audit subscriber on `{prefix}.audit.>`.
        // Act: GET /admin/policy/inspect with allow scenario.
        // Assert: no `{prefix}.audit.allow.request.*` message; only admin audit if applicable.
        unimplemented!("inspect is side-effect free on request audit stream");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_policy_inspect_missing_required_query_param_returns_400() {
        // Arrange: admin JWT; omit subject or method query param.
        // Act: GET /admin/policy/inspect?method=tools/list (no subject).
        // Assert: HTTP 400 with validation error body.
        unimplemented!("inspect validates required query parameters");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn get_policy_inspect_tool_query_optional_for_non_tools_call() {
        // Arrange: rule keyed on method tools/list only.
        // Act: GET /admin/policy/inspect?subject=...&method=tools/list (no tool).
        // Assert: decision returned; tool field ignored or absent in evaluation.
        unimplemented!("tool query param required only for tools/call inspect");
    }
}

mod role_check {
    //! Admin endpoints require `admin` role in JWT.

    use super::harness::{ADMIN_LISTEN_ADDR, ADMIN_ROLE, RELOAD_PATH, STATUS_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn admin_endpoint_without_admin_role_returns_403() {
        // Arrange: JWT with valid sub but roles omitting admin.
        // Act: POST /admin/reload or GET /admin/status without admin role.
        // Assert: HTTP 403; no reload or status body.
        let _ = (ADMIN_LISTEN_ADDR, ADMIN_ROLE, RELOAD_PATH);
        unimplemented!("403 when admin role absent from JWT");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn admin_endpoint_with_admin_role_succeeds() {
        // Arrange: JWT including roles: ["admin"] (or equivalent claim).
        // Act: GET /admin/status.
        // Assert: HTTP 200; status body returned.
        let _ = STATUS_PATH;
        unimplemented!("admin role grants access to admin surface");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn missing_jwt_on_admin_endpoint_returns_401_or_403() {
        // Arrange: admin_api_enabled gateway; no Authorization header.
        // Act: POST /admin/reload.
        // Assert: HTTP 401 or 403 per Block G auth policy; no side effects.
        unimplemented!("unauthenticated admin requests rejected");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn mcp_ingress_jwt_without_admin_role_cannot_call_admin_paths() {
        // Arrange: same JWT accepted on `{prefix}.gateway.request.>` but lacks admin role.
        // Act: reuse token against POST /admin/reload.
        // Assert: HTTP 403; MCP ingress still works for non-admin traffic.
        unimplemented!("ingress credentials do not imply admin role");
    }
}

mod audit {
    //! Admin actions emit dedicated `admin.{action}` audit envelopes.

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn reload_emits_admin_reload_audit_envelope() {
        // Arrange: JetStream subscriber on `{prefix}.audit.admin.>` or admin-specific subject.
        // Act: POST /admin/reload with admin JWT.
        // Assert: envelope action == admin.reload; distinct from request.allow/deny subjects.
        unimplemented!("admin.reload audit envelope on admin action");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn rollback_emits_admin_bundle_rollback_audit_envelope() {
        // Arrange: audit capture for admin actions.
        // Act: POST /admin/bundle/rollback.
        // Assert: envelope action == admin.bundle.rollback with previous/new sha fields.
        unimplemented!("admin.bundle.rollback audit envelope");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn admin_audit_separate_from_regular_request_envelopes() {
        // Arrange: concurrent MCP request generating request audit; admin reload in parallel.
        // Act: compare published subjects and schema action fields.
        // Assert: admin.{action} never uses request.* audit subject grammar.
        unimplemented!("admin audit channel isolated from MCP request audits");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn policy_inspect_emits_admin_policy_inspect_audit_envelope() {
        // Arrange: audit subscriber; admin JWT.
        // Act: GET /admin/policy/inspect?subject=...&method=...&tool=...
        // Assert: admin.policy.inspect envelope with decision and trace_id echo.
        unimplemented!("admin.policy.inspect audit on dry-run");
    }
}

mod feature_flag {
    //! Admin routes are not mounted when `admin_api_enabled=false`.

    use super::harness::{ADMIN_LISTEN_ADDR, RELOAD_PATH, STATUS_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn admin_routes_not_mounted_when_admin_api_disabled() {
        // Arrange: GatewaySettings with admin_api_enabled=false (production default).
        // Act: GET /admin/status, POST /admin/reload against admin listen addr.
        // Assert: connection refused or HTTP 404; no admin handler registered.
        let _ = (ADMIN_LISTEN_ADDR, STATUS_PATH, RELOAD_PATH);
        unimplemented!("admin_api_enabled=false omits admin router");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn health_probes_remain_available_when_admin_api_disabled() {
        // Arrange: admin_api_enabled=false; operational listener on :8080 for probes.
        // Act: GET /healthz and /readyz.
        // Assert: probe endpoints respond; admin paths absent on same or separate listener per spec.
        unimplemented!("probe surface independent of admin feature flag");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn admin_routes_mounted_when_admin_api_enabled() {
        // Arrange: admin_api_enabled=true; valid admin JWT.
        // Act: GET /admin/status.
        // Assert: HTTP 200 (or auth error if JWT missing); route exists (not 404).
        unimplemented!("admin_api_enabled=true registers admin router");
    }
}

mod mtls {
    //! Optional mTLS on the admin port when `admin_require_mtls=true`.

    use super::harness::{ADMIN_LISTEN_ADDR, RELOAD_PATH};

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn missing_client_cert_returns_401_when_mtls_required() {
        // Arrange: admin_require_mtls=true; plain HTTP client without client cert.
        // Act: POST /admin/reload with valid admin JWT but no TLS client certificate.
        // Assert: HTTP 401 before JWT middleware evaluates role.
        let _ = (ADMIN_LISTEN_ADDR, RELOAD_PATH);
        unimplemented!("401 when client cert missing and admin_require_mtls=true");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn valid_client_cert_allows_admin_request_when_mtls_required() {
        // Arrange: admin_require_mtls=true; HTTP client with trusted client cert + admin JWT.
        // Act: GET /admin/status.
        // Assert: HTTP 200; mTLS handshake succeeds.
        unimplemented!("mTLS satisfied admin request proceeds to JWT role check");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when admin API lands"]
    async fn mtls_not_required_when_admin_require_mtls_false() {
        // Arrange: admin_require_mtls=false (default); admin_api_enabled=true.
        // Act: GET /admin/status over TLS or plain per listener config without client cert.
        // Assert: request reaches JWT role gate; not rejected for missing client cert.
        unimplemented!("mTLS optional when admin_require_mtls=false");
    }
}

mod ctl_cli {
    //! `trogon-gateway-ctl` operator CLI (Block G item 2) — offline verbs and config introspection.

    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    use nkeys::KeyPair;
    use trogon_mcp_gateway::bundle::{
        build_tar, hash_member, manifest_digest_bytes, signature_path, BundleArchive, HOST_TARGET_WIT,
        MANIFEST_FILENAME,
    };

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root")
            .to_path_buf()
    }

    fn temp_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("trogon-gateway-ctl-{label}-{nanos}"))
    }

    fn ctl_binary() -> PathBuf {
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_trogon-gateway-ctl") {
            return PathBuf::from(path);
        }
        std::env::current_exe()
            .ok()
            .and_then(|current| {
                current
                    .parent()
                    .and_then(|deps| deps.parent())
                    .map(|debug| debug.join("trogon-gateway-ctl"))
            })
            .filter(|path| path.exists())
            .unwrap_or_else(|| workspace_root().join("target/debug/trogon-gateway-ctl"))
    }

    fn ctl_command() -> Command {
        let bin = ctl_binary();
        if bin.exists() {
            return Command::new(bin);
        }
        let mut cmd = Command::new("cargo");
        cmd.current_dir(workspace_root())
            .args(["run", "-p", "trogon-gateway-ctl", "--quiet", "--"]);
        cmd
    }

    fn signed_tar(kp: &KeyPair, cel_body: &[u8]) -> Vec<u8> {
        let hash = hash_member(cel_body);
        let manifest = format!(
            r#"
name = "acme/demo"
version = "1.0.0"
target_wit = "{HOST_TARGET_WIT}"
min_gateway_version = "0.0.1"
author = "platform"
created_at = "2026-05-28T00:00:00Z"
description = "demo"

[signing]
nkey_pub = "{}"

[[programs]]
id = "rule"
path = "policies/rule.cel"
sha256 = "{hash}"
class = "ingress_gate"
effect = "allow"
priority = 1
"#,
            kp.public_key()
        );
        let manifest_bytes = manifest.into_bytes();
        let sig = kp.sign(&manifest_digest_bytes(&manifest_bytes)).expect("sign");
        let mut archive = BundleArchive::default();
        archive.insert(MANIFEST_FILENAME, manifest_bytes);
        archive.insert("policies/rule.cel", cel_body.to_vec());
        archive.insert(signature_path(), sig);
        build_tar(&archive)
    }

    #[test]
    fn bundle_validate_prints_manifest_summary_for_signed_fixture() {
        let kp = KeyPair::new_user();
        let tar = signed_tar(&kp, b"true");
        let bundle_path = temp_path("bundle.tar");
        fs::write(&bundle_path, tar).expect("write bundle");
        let keys_path = temp_path("trusted.keys");
        fs::write(&keys_path, format!("{}\n", kp.public_key())).expect("write keys");

        let output = ctl_command()
            .args([
                "bundle",
                "validate",
                bundle_path.to_str().expect("path"),
                "--trusted-keys",
                keys_path.to_str().expect("keys"),
            ])
            .output()
            .expect("ctl bundle validate");

        let _ = fs::remove_file(bundle_path);
        let _ = fs::remove_file(keys_path);

        assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"result\":\"VALID\""));
        assert!(stdout.contains("acme/demo"));
    }

    #[test]
    fn bundle_validate_exits_nonzero_for_untrusted_signer() {
        let kp = KeyPair::new_user();
        let tar = signed_tar(&kp, b"true");
        let bundle_path = temp_path("bundle-invalid.tar");
        fs::write(&bundle_path, tar).expect("write bundle");
        let other = KeyPair::new_user();
        let keys_path = temp_path("trusted-other.keys");
        fs::write(&keys_path, format!("{}\n", other.public_key())).expect("write keys");

        let output = ctl_command()
            .args([
                "bundle",
                "validate",
                bundle_path.to_str().expect("path"),
                "--trusted-keys",
                keys_path.to_str().expect("keys"),
            ])
            .output()
            .expect("ctl bundle validate");

        let _ = fs::remove_file(bundle_path);
        let _ = fs::remove_file(keys_path);

        assert!(!output.status.success());
        assert_eq!(output.status.code(), Some(1));
    }

    #[test]
    fn policy_dry_run_reports_allow_and_deny() {
        let policy_path = temp_path("policy.cel");
        fs::write(&policy_path, "jwt.sub == \"alice\"").expect("policy write");
        let allow_input_path = temp_path("allow.json");
        fs::write(
            &allow_input_path,
            r#"{"identity":{"caller_sub":"alice","source":"jwt"},"jsonrpc_method":"tools/call","tool_name":"deploy"}"#,
        )
        .expect("allow input write");
        let allow = ctl_command()
            .args([
                "policy",
                "dry-run",
                "--policy",
                policy_path.to_str().expect("policy"),
                "--input",
                allow_input_path.to_str().expect("input"),
            ])
            .output()
            .expect("policy dry-run allow");
        assert!(allow.status.success());
        assert!(String::from_utf8_lossy(&allow.stdout).contains("\"decision\":\"allow\""));

        let deny_input_path = temp_path("deny.json");
        fs::write(
            &deny_input_path,
            r#"{"identity":{"caller_sub":"bob","source":"jwt"},"jsonrpc_method":"tools/list"}"#,
        )
        .expect("deny input write");
        let deny = ctl_command()
            .args([
                "policy",
                "dry-run",
                "--policy",
                policy_path.to_str().expect("policy"),
                "--input",
                deny_input_path.to_str().expect("input"),
            ])
            .output()
            .expect("policy dry-run deny");

        let _ = fs::remove_file(policy_path);
        let _ = fs::remove_file(allow_input_path);
        let _ = fs::remove_file(deny_input_path);

        assert!(deny.status.success());
        assert!(String::from_utf8_lossy(&deny.stdout).contains("\"decision\":\"deny\""));
    }

    #[test]
    fn config_show_emits_gateway_settings_json() {
        let output = ctl_command()
            .args(["config", "show"])
            .output()
            .expect("config show");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("\"mcp_prefix\""));
        assert!(stdout.contains("\"audit_stream_name\""));
    }

    #[tokio::test]
    #[ignore = "requires live NATS broker with MCP audit stream"]
    async fn trace_request_id_queries_jetstream_audit_stream() {
        // Arrange: published audit envelope with known request_id on MCP_AUDIT stream.
        // Act: trogon-gateway-ctl trace req-123
        // Assert: JSON events[] contains matching envelope chronologically.
        unimplemented!("trace JetStream integration");
    }

    #[tokio::test]
    #[ignore = "requires live NATS broker publishing audit events"]
    async fn audit_tail_streams_audit_subject_events() {
        // Arrange: gateway publishing to {prefix}.audit.>
        // Act: trogon-gateway-ctl audit tail --max 1
        // Assert: stdout JSON lines with subject + envelope.
        unimplemented!("audit tail integration");
    }
}
