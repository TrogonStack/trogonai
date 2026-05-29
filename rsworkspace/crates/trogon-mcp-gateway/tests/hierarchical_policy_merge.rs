//! Hierarchical policy merge integration tests (scaffold).
//!
//! Phase 2 merges policy bundles across `org -> tenant -> server-group -> server -> method`
//! with sticky deny, explicit allow, and default deny per ADR 0013. These tests define the
//! contract for end-to-end merge evaluation once `trogon_mcp_gateway::policy::hierarchical`
//! lands; bodies stay skeletal until Block E item 6 is implemented.
//!
//! Cross-refs:
//! - `docs/adr/0013-hierarchical-policy-merge.md`
//! - `docs/identity/hierarchical-policy-merge.md`
//! - `MCP_GATEWAY_PLAN.md` Block E item 6
//!
//! Once implemented, each test will seed KV policy bundles at the relevant hierarchy levels,
//! drive a gateway request (or direct merge evaluation), and assert allow/deny plus audit
//! envelope fields (`rules_fired`, `policy_merge.expression_hash`).
//!
//! Harness pattern (see `e2e_nats_forward.rs`): `mcp_nats::Config`, `McpPrefix`,
//! `trogon_nats::NatsAuth`, `GatewaySettings`, `AllowAllPermissionChecker`, `TraceStore`.

mod org_baseline_allow {
    //! Org-only allow with empty downstream layers.

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn only_org_allow_permits_request() {
        // Arrange: org bundle with allow rule; tenant, server-group, server, method unset.
        // Act: evaluate via trogon_mcp_gateway::policy::hierarchical::MergeEngine (proposed).
        // Assert: decision == Allow; no deny layers fired.
        unimplemented!("org-only allow should permit when no downstream deny exists");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn org_allow_skips_empty_tenant_and_server_layers() {
        // Arrange: org allow; KV keys for tenant/server-group/server/method absent.
        // Act: merge walk org -> tenant -> server-group -> server -> method.
        // Assert: empty levels contribute false disjuncts; merged allow side satisfied.
        unimplemented!("skipped empty layers must not block org allow");
    }
}

mod sticky_deny {
    //! Deny is sticky across hierarchy levels — allow at any layer cannot override deny.

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn tenant_deny_overrides_org_allow() {
        // Arrange: org allow + tenant deny for same request context.
        // Act: hierarchical merge per ADR 0013 hybrid semantics.
        // Assert: decision == Deny; error cites tenant deny policy_id.
        unimplemented!("tenant deny must stick over org allow");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn server_group_deny_overrides_method_allow() {
        // Arrange: server-group deny + method-level allow rule.
        // Act: merge with precedence org -> tenant -> server-group -> server -> method.
        // Assert: decision == Deny; method allow does not widen server-group deny.
        unimplemented!("server-group deny must stick over method allow");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn org_deny_sticks_over_all_lower_layer_allows() {
        // Arrange: org deny; tenant, server-group, server, and method all allow.
        // Act: hierarchical merge.
        // Assert: decision == Deny; org deny short-circuits despite downstream allows.
        unimplemented!("org deny must stick over every lower-layer allow");
    }
}

mod default_deny {
    //! Default deny when no layer contributes an allow.

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn empty_hierarchy_defaults_to_deny() {
        // Arrange: no bundles at org, tenant, server-group, server, or method.
        // Act: merge evaluation in production enforce mode.
        // Assert: decision == Deny; JSON-RPC -32108 no_policy.
        unimplemented!("absence of rules is not allow; default deny");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn no_explicit_allow_side_denies_without_matching_deny() {
        // Arrange: layers present but none emit allow predicates (config-only bundles).
        // Act: hierarchical merge.
        // Assert: decision == Deny even when no deny rule matched.
        unimplemented!("empty allow side denies per ADR 0013 default");
    }
}

mod shallow_cel_merge {
    //! Shallow field-level merge: later layers override earlier CEL/config fields.

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn method_layer_cel_overrides_server_group_allow() {
        // Arrange: org/tenant/server-group allow with CEL predicate A; method allow with
        // conflicting CEL predicate B on the same merged field.
        // Act: shallow merge producing single merged expression H(M).
        // Assert: method-layer CEL wins; evaluation uses predicate B not A.
        unimplemented!("method layer must override server-group CEL on field collision");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn server_group_overrides_tenant_cel_on_shallow_merge() {
        // Arrange: tenant and server-group both allow with different CEL on same config key.
        // Act: shallow merge walk.
        // Assert: server-group expression replaces tenant wholesale (no deep merge).
        unimplemented!("server-group shallow merge replaces tenant CEL field");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn merged_allow_side_or_across_non_conflicting_layers() {
        // Arrange: org allow predicate matches; tenant adds non-colliding allow predicate.
        // Act: merge authorization OR semantics across layers.
        // Assert: both predicates compiled into merged allow side; either match permits.
        unimplemented!("non-colliding allow predicates OR across layers");
    }
}

mod audit_rules_fired {
    //! Audit envelope lists every contributing rule across hierarchy levels.

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn rules_fired_lists_org_and_tenant_contributors() {
        // Arrange: org + tenant bundles both compiled into merged expression M.
        // Act: gateway request through policy gate; capture audit envelope.
        // Assert: rules_fired contains every policy_id compiled into M at merge time.
        unimplemented!("rules_fired must list all contributing policy_ids");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn rules_fired_spans_all_merged_hierarchy_levels() {
        // Arrange: bundles at org, tenant, server-group, server, and method.
        // Act: merge + evaluate; publish audit deny or allow envelope.
        // Assert: rules_fired includes policy_id from each populated level.
        unimplemented!("rules_fired must span every hierarchy level in M");
    }

    #[tokio::test]
    #[ignore = "scaffold; implement when hierarchical merge per ADR 0013 lands"]
    async fn audit_envelope_carries_policy_merge_expression_hash() {
        // Arrange: multi-layer merge producing stable hash H(M).
        // Act: gateway audit publish on policy decision.
        // Assert: envelope policy_merge.expression_hash == H(M); offline replay matches.
        unimplemented!("audit envelope must carry policy_merge.expression_hash");
    }
}
