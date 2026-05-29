//! Integration scaffold for schema-driven redaction on the MCP gateway forward path.
//!
//! Phase 2 attaches JSONPath redaction rules to `inputSchema` / `outputSchema` (YAML in the
//! bundle). Rules run before audit, anomaly, and low-trust egress. Cross-refs:
//! `MCP_GATEWAY_PLAN.md` Block E item 5 and the "Redaction" section; scaffold types in
//! `trogon_mcp_gateway::redaction`.
//!
//! Once implemented, these tests verify:
//! - `hash`, `drop`, and `mask` operators on request and response payloads
//! - nested JSONPath targeting and deterministic multi-rule application order
//! - audit envelope `rewrites` recording every fired rule in order
//! - fail-closed `-32104` / `schema_unknown` when the schema cache misses

use trogon_mcp_gateway::redaction::{
    JsonPath, RedactionAction, RedactionRule, RedactionRuleset, redact,
};

/// Touch scaffold redaction types so the integration file compiles before the gateway wires them.
#[allow(dead_code)]
fn touch_scaffold_redaction_types() {
    let path = JsonPath::parse("$.params.token").expect("valid path");
    let rule = RedactionRule {
        path,
        action: RedactionAction::Hash,
    };
    let ruleset = RedactionRuleset::builder().rule(rule).build();
    let mut doc = serde_json::json!({ "params": { "token": "secret" } });
    redact(&mut doc, &ruleset);
}

mod hash_redaction {
    //! `op=hash` replaces sensitive fields with a stable digest before backend egress.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn hash_replaces_token_before_backend_egress() {
        // Arrange: NATS harness (`McpPrefix`, `GatewaySettings`, backend subscriber on
        // `{prefix}.server.{id}.tools.call`) with inputSchema rule
        // `path=$.params.token, op=hash` attached via bundle / schema cache.
        // Act: client `tools/call` with `{ "params": { "token": "sk-live-..." } }`.
        // Assert: backend receives hashed token, not the plaintext secret.
        unimplemented!("hash op on $.params.token before backend egress");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn hash_audit_rewrite_records_path_and_op() {
        // Arrange: same harness; subscribe to `mcp.audit.>` or configured audit stream.
        // Act: allowed `tools/call` that triggers hash redaction on `$.params.token`.
        // Assert: audit envelope `rewrites` contains `[{ "path": "$.params.token", "op": "hash" }]`.
        unimplemented!("audit envelope rewrites entry for hash rule");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn hash_is_deterministic_for_identical_input() {
        // Arrange: two identical requests with the same token value and hash rule.
        // Act: forward both through the gateway.
        // Assert: backend sees the same digest for the same plaintext (stable hash, not random).
        unimplemented!("deterministic hash digest for repeated token values");
    }
}

mod drop_redaction {
    //! `op=drop` removes fields from the egress payload entirely (not JSON null).

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn drop_removes_field_from_backend_egress_payload() {
        // Arrange: rule `path=$.params.internal_note, op=drop` on tool inputSchema.
        // Act: `tools/call` including `internal_note` in params.
        // Assert: backend JSON body has no `internal_note` key at any nesting level matched.
        unimplemented!("drop op removes matched field from egress payload");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn drop_does_not_emit_null_placeholder() {
        // Arrange: drop rule on a nested param field.
        // Act: forward request through gateway.
        // Assert: dropped key is absent, not present as `null` (audit/anomaly cannot distinguish null from missing).
        unimplemented!("drop must remove key entirely, not substitute null");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn drop_audit_rewrite_records_path_and_op() {
        // Arrange: audit consumer on gateway audit stream.
        // Act: request that fires a drop rule.
        // Assert: `rewrites` includes `{ "path": "<matched path>", "op": "drop" }`.
        unimplemented!("audit envelope records drop rewrite");
    }
}

mod mask_redaction {
    //! `op=mask` replaces sensitive scalar fields with the literal `"***"`.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn mask_replaces_field_with_literal_stars() {
        // Arrange: rule `path=$.params.api_key, op=mask` on inputSchema.
        // Act: `tools/call` with plaintext api_key in params.
        // Assert: backend receives `"***"` at `$.params.api_key`.
        unimplemented!("mask op replaces field with ***");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn mask_audit_rewrite_records_path_and_op() {
        // Arrange: audit stream subscriber.
        // Act: masked request allowed through gateway.
        // Assert: audit `rewrites` contains `{ "path": "$.params.api_key", "op": "mask" }`.
        unimplemented!("audit envelope records mask rewrite");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn mask_does_not_leak_original_in_audit_payload() {
        // Arrange: mask rule on a sensitive param; full audit envelope capture enabled.
        // Act: forward masked request.
        // Assert: audit body does not contain the original plaintext value.
        unimplemented!("masked plaintext must not appear in audit payload");
    }
}

mod bidirectional_redaction {
    //! Redaction applies on inbound request params and outbound response results.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn request_path_rules_apply_to_tools_call_params() {
        // Arrange: inputSchema rule on `$.params.connection_string` with `op=hash`.
        // Act: client `tools/call` with connection string in params.
        // Assert: backend request payload is redacted per request-path rules.
        unimplemented!("request-path redaction on tools/call params");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn response_path_rules_apply_to_tools_call_result() {
        // Arrange: outputSchema rule on `$.result.rows[*].ssn` with `op=mask`; backend returns PII.
        // Act: client receives gateway-forwarded `tools/call` reply.
        // Assert: client-visible result has masked SSN fields per response-path rules.
        unimplemented!("response-path redaction on tools/call result");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn request_and_response_rules_both_fire_on_single_call() {
        // Arrange: hash rule on request param and mask rule on response field for the same tool.
        // Act: round-trip `tools/call`.
        // Assert: backend sees hashed request; client sees masked response; audit lists both rewrites.
        unimplemented!("both request and response redaction on one tools/call");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn response_redaction_audit_records_direction() {
        // Arrange: response-only redaction rule; capture audit envelopes for request and response phases.
        // Act: successful `tools/call` with sensitive result fields.
        // Assert: response-direction audit envelope includes response-path `rewrites`.
        unimplemented!("response-phase audit records response redaction rewrites");
    }
}

mod schema_cache_gate {
    //! Fail-closed when inputSchema is unknown: block request rather than forward un-redacted.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn schema_cache_miss_returns_schema_unknown_error() {
        // Arrange: tool with redaction rules but no cached inputSchema and fetch failure.
        // Act: client `tools/call` for that tool.
        // Assert: JSON-RPC error code `-32104`, message `schema_unknown` (per MCP_GATEWAY_PLAN.md §6).
        unimplemented!("-32104 schema_unknown on schema cache miss");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn schema_unknown_error_includes_trace_and_tool_context() {
        // Arrange: schema miss for `github::create_issue`.
        // Act: blocked `tools/call`.
        // Assert: error `data` shape `{ trace_id, server_id, tool }` with native tool name.
        unimplemented!("schema_unknown data carries trace_id, server_id, tool");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn schema_unknown_does_not_forward_to_backend() {
        // Arrange: backend subscriber on server lane; schema cache empty for gated tool.
        // Act: client request that would require redaction validation.
        // Assert: backend receives no message; client gets `-32104` instead.
        unimplemented!("schema miss blocks forward rather than silent pass-through");
    }
}

mod nested_jsonpath {
    //! Nested JSONPath selectors such as `$.params.user.email` target deep fields.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn nested_path_masks_deep_email_field() {
        // Arrange: rule `path=$.params.user.email, op=mask` on tool inputSchema.
        // Act: `tools/call` with nested `{ "params": { "user": { "email": "alice@acme.com" } } }`.
        // Assert: backend sees `"***"` at `$.params.user.email`; sibling fields unchanged.
        unimplemented!("nested JSONPath $.params.user.email mask");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn nested_path_hash_applies_without_affecting_parent_object() {
        // Arrange: hash rule on `$.params.credentials.token`.
        // Act: forward nested credential object.
        // Assert: only the leaf token is hashed; parent `credentials` object structure preserved.
        unimplemented!("nested hash leaves parent object intact");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn nested_path_drop_removes_leaf_only() {
        // Arrange: drop rule on `$.params.metadata.internal_id`.
        // Act: forward params with other metadata keys present.
        // Assert: `internal_id` absent; remaining `metadata` keys forwarded unchanged.
        unimplemented!("nested drop removes matched leaf only");
    }
}

mod rule_application_order {
    //! Multi-rule application follows stable, deterministic iteration order.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn multi_rule_application_order_is_stable_across_requests() {
        // Arrange: ruleset with mask on `$.params.a`, hash on `$.params.b`, drop on `$.params.c`
        // declared in YAML order.
        // Act: two identical requests through the gateway.
        // Assert: same final payload shape and same rewrite sequence both times.
        unimplemented!("stable multi-rule iteration order");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn overlapping_rules_apply_in_yaml_declaration_order() {
        // Arrange: rules whose paths could interact (e.g. parent object and nested field).
        // Act: single `tools/call` triggering multiple rules.
        // Assert: effective payload matches deterministic order documented in bundle YAML.
        unimplemented!("YAML declaration order governs overlapping rules");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn rule_order_matches_ruleset_builder_iteration() {
        // Arrange: `RedactionRuleset::builder()` with three rules in known order.
        // Act: integration forward using equivalent bundle attachment.
        // Assert: application order matches `ruleset.rules()` slice order.
        unimplemented!("gateway rule order matches RedactionRuleset iteration");
    }
}

mod audit_rewrites_envelope {
    //! Audit envelope `rewrites` lists every fired rule in application order.

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn audit_rewrites_lists_every_fired_rule_in_order() {
        // Arrange: three rules (hash, mask, drop) on one tool; audit stream consumer.
        // Act: `tools/call` where all three match.
        // Assert: `rewrites` array length 3 with `{ path, op }` entries in firing order.
        unimplemented!("audit rewrites lists all fired rules in order");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn audit_rewrites_omits_rules_that_did_not_match() {
        // Arrange: rules on optional params; request omits one matched path.
        // Act: forward partial payload.
        // Assert: `rewrites` includes only rules whose paths resolved in the document.
        unimplemented!("audit rewrites excludes non-matching rules");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn audit_rewrites_merge_with_subject_route_rewrites() {
        // Arrange: gateway forward that also records subject rewrite metadata in audit.
        // Act: redacted `tools/call` allowed through.
        // Assert: payload `rewrites` includes redaction entries without dropping route rewrites.
        unimplemented!("redaction rewrites merge with forward audit extras");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when schema-driven redaction per MCP_GATEWAY_PLAN.md Block E item 5 lands"]
    async fn audit_rewrites_use_jsonpath_and_op_wire_format() {
        // Arrange: capture audit envelope for a single hash rule.
        // Act: allowed redacted request.
        // Assert: each rewrite entry uses `{ "path": "<jsonpath>", "op": "<action>" }` per plan § Audit.
        unimplemented!("rewrites wire format matches MCP_GATEWAY_PLAN.md audit schema");
    }
}
