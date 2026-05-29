//! Bundle load and hot-reload integration tests (scaffold).
//!
//! Phase 2 loads WASM policy bundles from disk or KV per ADR 0010: cold start readiness,
//! parse-error retention of the last good bundle, fs-watcher hot swap with in-flight drain,
//! `/readyz` bundle digest, atomic pointer flip, operator rollback, NKey signature gate,
//! and load-time CEL compilation. Bodies stay skeletal until Block C item 5 lands.
//!
//! Cross-refs:
//! - `docs/adr/0010-bundle-format.md`
//! - `docs/identity/howto-write-bundle.md`
//! - `MCP_GATEWAY_PLAN.md` Block C item 5
//!
//! Once implemented, each test will stand up a gateway with a temp bundle path (or KV stub),
//! drive HTTP `/readyz` and policy requests, and assert readiness, metrics, and request
//! routing against the active bundle digest.
//!
//! Harness pattern (see `e2e_nats_forward.rs`): `mcp_nats::Config`, `McpPrefix`,
//! `trogon_nats::NatsAuth`, `GatewaySettings`, `AllowAllPermissionChecker`, `TraceStore`.

mod cold_start {
    //! Boot without a bundle file; readiness flips after a valid bundle appears.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn readyz_returns_503_when_bundle_absent_at_boot() {
        // Arrange: gateway starts with configured bundle path missing on disk.
        // Act: GET /readyz before any bundle is published.
        // Assert: HTTP 503; body indicates bundle not loaded.
        unimplemented!("boot without bundle must not report ready");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn readyz_flips_to_200_after_valid_bundle_appears() {
        // Arrange: gateway running with empty bundle path; write valid manifest + CEL pack.
        // Act: fs notify or poll until loader accepts bundle; GET /readyz.
        // Assert: HTTP 200; ready == true.
        unimplemented!("valid bundle on disk must flip readiness to true");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn gateway_serves_no_policy_until_first_good_bundle_loads() {
        // Arrange: cold start with absent bundle; ingress client connected.
        // Act: policy-gated JSON-RPC before bundle lands.
        // Assert: request denied or not-ready per gateway contract (not last-good fallback).
        unimplemented!("no prior bundle means no policy surface until first load");
    }
}

mod parse_error {
    //! Invalid bundle rejected; gateway keeps last good bundle and emits parse_error metric.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn invalid_yaml_rejected_keeps_last_good_bundle() {
        // Arrange: gateway active with bundle v1; overwrite path with malformed YAML.
        // Act: watcher triggers reload; drive policy request.
        // Assert: still evaluates v1 rules; /readyz still 200 with v1 digest.
        unimplemented!("YAML parse failure must not replace active bundle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn unknown_cel_function_rejected_keeps_last_good_bundle() {
        // Arrange: active bundle with known CEL; publish bundle referencing undefined CEL fn.
        // Act: reload attempt; policy request.
        // Assert: prior bundle remains active for evaluation.
        unimplemented!("CEL unknown function must reject bundle without dropping last good");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn bundle_parse_error_metric_emitted_with_path_and_reason() {
        // Arrange: metrics sink or test exporter; publish invalid bundle at known path.
        // Act: failed reload.
        // Assert: counter/histogram `bundle.parse_error` with labels path, reason.
        unimplemented!("parse failures must increment bundle.parse_error with path and reason");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn readyz_digest_unchanged_after_parse_error() {
        // Arrange: /readyz records bundle.sha256 for v1; corrupt v2 on disk.
        // Act: failed reload; GET /readyz.
        // Assert: bundle.sha256 still matches v1 canonical digest.
        unimplemented!("failed reload must not change readyz bundle digest");
    }
}

mod hot_reload {
    //! Fs-watcher promotes new bundle for new requests; in-flight work drains on prior digest.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn file_edit_activates_new_bundle_for_subsequent_requests() {
        // Arrange: v1 bundle active; in-flight none; atomically replace with v2 on disk.
        // Act: watcher reload; new policy-gated request with v2-only rule marker.
        // Assert: evaluation uses v2 policy_id / digest in audit envelope.
        unimplemented!("post-reload requests must see newly loaded bundle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn in_flight_request_completes_with_old_bundle_after_reload() {
        // Arrange: slow handler or held request on v1; trigger v2 reload mid-flight.
        // Act: complete in-flight request; then send fresh request.
        // Assert: in-flight audit references v1 digest; subsequent request references v2.
        unimplemented!("in-flight evaluations must drain on pre-reload bundle digest");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn watcher_debounce_coalesces_rapid_writes_to_single_reload() {
        // Arrange: rapid sequential writes to bundle path within debounce window.
        // Act: observe loader reload count or metric.
        // Assert: single activation of final on-disk bytes.
        unimplemented!("debounced watcher must not thrash reload on partial writes");
    }
}

mod sha256 {
    //! `/readyz` JSON exposes canonical bundle SHA-256.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn readyz_json_includes_bundle_sha256_field() {
        // Arrange: known bundle bytes on disk; gateway loaded.
        // Act: GET /readyz; parse JSON body.
        // Assert: `bundle.sha256` present and lowercase hex of canonical signed manifest bytes.
        unimplemented!("readyz must expose bundle.sha256 per ADR 0010");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn readyz_sha256_updates_after_successful_hot_reload() {
        // Arrange: v1 loaded; replace with v2 of different content.
        // Act: successful reload; GET /readyz.
        // Assert: bundle.sha256 changes to v2 digest.
        unimplemented!("digest on readyz must track active bundle after reload");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn readyz_sha256_matches_oci_tag_suffix_convention() {
        // Arrange: bundle with known manifest.toml canonical bytes per ADR 0010.
        // Act: compute expected sha256-12 tag suffix; compare /readyz field.
        // Assert: readyz digest consistent with mcp-policy@semver+sha256-12 promotion alias.
        unimplemented!("readyz digest must align with ADR 0010 canonical manifest hashing");
    }
}

mod atomic_swap {
    //! Active bundle pointer flips atomically; no request observes a half-applied bundle.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn concurrent_requests_never_see_partial_manifest() {
        // Arrange: large bundle reload; concurrent policy requests during swap window.
        // Act: sample audit policy_bundle_digest per request.
        // Assert: every digest is either full v1 or full v2, never mixed manifest fields.
        unimplemented!("atomic swap must not expose torn bundle state");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn reload_publishes_new_pointer_only_after_full_compile() {
        // Arrange: instrument loader stages; bundle with CEL requiring compile.
        // Act: reload v2.
        // Assert: active pointer unchanged until compile + verify complete.
        unimplemented!("pointer flip happens only after full load pipeline succeeds");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn failed_mid_pipeline_reload_leaves_prior_pointer() {
        // Arrange: v1 active; v2 fails signature after parse.
        // Act: attempted reload.
        // Assert: active pointer still v1; no torn v2 manifest visible to handlers.
        unimplemented!("failed pipeline must not partially publish new bundle");
    }
}

mod rollback {
    //! Operator-initiated rollback restores previous bundle via signal or admin endpoint.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn admin_endpoint_rollback_restores_previous_bundle() {
        // Arrange: v1 then v2 active; admin rollback API or config pin.
        // Act: POST rollback (proposed); policy request.
        // Assert: evaluation and /readyz digest match v1.
        unimplemented!("admin rollback must reactivate prior bundle digest");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn signal_rollback_restores_previous_bundle() {
        // Arrange: v1 then v2; send operator signal (SIGHUP or documented signal).
        // Act: wait for handler; GET /readyz.
        // Assert: bundle.sha256 reverts to v1.
        unimplemented!("signal rollback must restore last promoted bundle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn rollback_emits_bundle_loaded_audit_with_old_and_new_ids() {
        // Arrange: audit subscriber; v1 -> v2 -> rollback to v1.
        // Act: rollback.
        // Assert: audit envelope bundle_loaded records old and new identifiers per ADR 0010.
        unimplemented!("rollback must audit bundle_loaded with prior and restored digests");
    }
}

mod signature {
    //! Unsigned bundles rejected when `require_signed_bundles=true`.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn unsigned_bundle_rejected_when_require_signed_true() {
        // Arrange: gateway config require_signed_bundles=true; unsigned manifest on disk.
        // Act: reload attempt.
        // Assert: bundle not activated; last good unchanged or cold 503 if none.
        unimplemented!("unsigned bundle must not activate when signing required");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn valid_nkey_signature_activates_bundle() {
        // Arrange: trusted signer pubkey in config; bundle signed per ADR 0010 NKey scheme.
        // Act: load bundle.
        // Assert: /readyz 200; bundle.sha256 matches signed manifest digest.
        unimplemented!("valid NKey signature must pass verification gate");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn tampered_manifest_rejected_keeps_last_good() {
        // Arrange: v1 signed and active; v2 with valid sig but tampered manifest body.
        // Act: reload.
        // Assert: v1 remains active; bundle.parse_error or bundle_rejected audit reason.
        unimplemented!("manifest tamper must fail verification without dropping v1");
    }
}

mod cel_compile {
    //! CEL programs compile at bundle load time; compile errors reject the bundle.

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn cel_compiled_at_load_not_per_request() {
        // Arrange: bundle with policies/*.cel; spy or loader hook counting compile calls.
        // Act: N identical policy requests after load.
        // Assert: compile invoked once at load; per-request path uses precompiled program.
        unimplemented!("CEL compile must happen at bundle load per ADR 0010");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn cel_syntax_error_rejects_bundle_at_load() {
        // Arrange: active v1; publish v2 with invalid CEL syntax in policies/*.cel.
        // Act: reload.
        // Assert: v1 still active; bundle rejected before pointer flip.
        unimplemented!("CEL syntax error at load must reject bundle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn cel_type_error_rejects_bundle_at_load() {
        // Arrange: CEL referencing fields absent from policy context type.
        // Act: load attempt at startup.
        // Assert: bundle not activated; gateway remains not ready or on last good.
        unimplemented!("CEL type error at compile time must reject bundle");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when bundle hot reload per ADR 0010 lands"]
    async fn all_cel_files_in_bundle_compiled_before_activation() {
        // Arrange: bundle with multiple policies/*.cel files.
        // Act: load.
        // Assert: activation only if every file compiles; partial compile never goes live.
        unimplemented!("every CEL file must compile before bundle becomes active");
    }
}
