//! Per-item CEL filtering for MCP catalog list responses.
//!
//! Phase 2 re-evaluates the merged authorization rule once per catalogue entry on
//! `tools/list`, `resources/list`, and `prompts/list` before egress. Denied items
//! are omitted; the JSON-RPC envelope is otherwise unchanged.
//!
//! Cross-references:
//! - `docs/adr/0015-tools-list-filtering.md` (accepted decision)
//! - `docs/identity/tools-list-filtering.md` (algorithm, CEL bindings, audit fields)
//! - `MCP_GATEWAY_PLAN.md` Block E item 2
//!
//! Unit cases below exercise `policy::list_filter` directly. NATS harness cases remain
//! `#[ignore]` until the gateway e2e stub lands (`e2e_nats_forward.rs` pattern).

use std::sync::Arc;

use trogon_mcp_gateway::authz::{AllowAllPermissionChecker, GatewayIdentity, IdentitySource};
use trogon_mcp_gateway::jwt::VerifiedJwtClaims;
use trogon_mcp_gateway::policy::hierarchical::{
    MergeEngine, PolicyEffect, PolicyLevel, PolicyRule, PolicyStore, ScopeBundle, ScopeConfig, ScopeKey,
};
use trogon_mcp_gateway::policy::list_filter::{
    self, ListFilterParams, ToolCandidate, AUDIT_FILTERED_BY_CEL,
};

fn sample_identity() -> GatewayIdentity {
    GatewayIdentity {
        tenant: Some("acme".into()),
        caller_sub: Some("alice".into()),
        issuer: None,
        jti: None,
        source: IdentitySource::Jwt,
    }
}

fn engine_with_rules(rules: Vec<PolicyRule>) -> MergeEngine {
    let store = Arc::new(PolicyStore::default());
    let engine = MergeEngine::new(Arc::clone(&store), true);
    engine.upsert_scope(
        ScopeKey {
            level: PolicyLevel::Org,
            tenant: None,
            server_group: None,
            server_id: None,
            method: None,
            tool: None,
        },
        ScopeBundle {
            rules,
            config: ScopeConfig::default(),
        },
    );
    engine
}

mod unit_list_filter {
    use super::*;

    #[test]
    fn cel_rule_denies_subset_of_tools() {
        let engine = engine_with_rules(vec![
            PolicyRule {
                policy_id: "org/allow-alice".into(),
                revision: 1,
                effect: PolicyEffect::Allow,
                priority: 0,
                cel: r#"jwt.sub == "alice""#.into(),
            },
            PolicyRule {
                policy_id: "org/deny-secret".into(),
                revision: 1,
                effect: PolicyEffect::Deny,
                priority: 0,
                cel: r#"mcp.tool.name == "secret""#.into(),
            },
        ]);
        let identity = sample_identity();
        let outcome = list_filter::filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-1",
                checker: Arc::new(AllowAllPermissionChecker),
                candidates: vec![
                    ToolCandidate {
                        name: "read_file".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "secret".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "search".into(),
                        input_schema: None,
                    },
                ],
            },
        );

        assert_eq!(outcome.kept.len(), 2);
        assert!(outcome.kept.iter().any(|tool| tool.name == "read_file"));
        assert!(outcome.kept.iter().any(|tool| tool.name == "search"));
        assert_eq!(outcome.audit_events.len(), 1);
        assert_eq!(outcome.audit_events[0].event, AUDIT_FILTERED_BY_CEL);
        assert_eq!(outcome.audit_events[0].tool_name, "secret");
    }

    #[test]
    fn empty_candidate_list_is_noop() {
        let engine = engine_with_rules(vec![]);
        let identity = sample_identity();
        let outcome = list_filter::filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-1",
                checker: Arc::new(AllowAllPermissionChecker),
                candidates: vec![],
            },
        );
        assert!(outcome.kept.is_empty());
        assert!(outcome.audit_events.is_empty());
    }

    #[test]
    fn list_filter_index_binding_keeps_even_indices_only() {
        let engine = engine_with_rules(vec![PolicyRule {
            policy_id: "org/even-index".into(),
            revision: 1,
            effect: PolicyEffect::Allow,
            priority: 0,
            cel: "response.list_filter_index % 2 == 0".into(),
        }]);
        let identity = sample_identity();
        let outcome = list_filter::filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-1",
                checker: Arc::new(AllowAllPermissionChecker),
                candidates: vec![
                    ToolCandidate {
                        name: "t0".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "t1".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "t2".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "t3".into(),
                        input_schema: None,
                    },
                ],
            },
        );

        assert_eq!(
            outcome
                .kept
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>(),
            vec!["t0", "t2"]
        );
    }
}

mod tools_list_partial_denial {
    #[tokio::test]
    #[ignore = "needs NATS harness with policy bundle and backend stub"]
    async fn cel_rule_denies_two_of_five_backend_tools_client_sees_three() {
        unimplemented!("wire NATS harness per ADR 0015");
    }
}

mod tools_list_edge_cases {
    #[tokio::test]
    #[ignore = "needs NATS harness with list filter enabled"]
    async fn empty_backend_tools_array_returns_empty_success() {
        unimplemented!("empty catalogue passthrough per docs/identity/tools-list-filtering.md §7.4");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness with list filter enabled"]
    async fn all_tools_denied_returns_empty_tools_array() {
        unimplemented!("all-denied shaping per ADR 0015 response shape invariant");
    }
}

mod list_filter_index {
    #[tokio::test]
    #[ignore = "needs NATS harness exposing per-evaluation index capture"]
    async fn cel_rule_evaluates_with_response_list_filter_index_per_item() {
        unimplemented!("response.list_filter_index binding per ADR 0015 evaluation contract");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness exposing per-evaluation index capture"]
    async fn list_filter_index_differs_across_sibling_catalog_entries() {
        unimplemented!("indexed per-item re-evaluation per docs/identity/tools-list-filtering.md");
    }
}

mod catalog_list_parity {
    #[tokio::test]
    #[ignore = "needs NATS harness comparing tools/call and tools/list policy paths"]
    async fn tools_list_per_item_cel_filter_matches_call_authorization_rule() {
        unimplemented!("tools/list parity per MCP_GATEWAY_PLAN Block E item 2");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness for resources/list shaping"]
    async fn resources_list_per_item_cel_filter_matches_call_authorization_rule() {
        unimplemented!("resources/list shaping per ADR 0015 catalog method table");
    }

    #[tokio::test]
    #[ignore = "needs NATS harness for prompts/list shaping"]
    async fn prompts_list_per_item_cel_filter_matches_call_authorization_rule() {
        unimplemented!("prompts/list shaping per ADR 0015 catalog method table");
    }
}

mod audit_filtered_count {
    #[tokio::test]
    #[ignore = "needs mock audit publisher on gateway ingress path"]
    async fn audit_envelope_records_catalog_tools_filtered_count() {
        unimplemented!("AuditEnvelope catalog_* fields per docs/identity/tools-list-filtering.md §8");
    }

    #[tokio::test]
    #[ignore = "needs mock audit publisher on gateway ingress path"]
    async fn mock_audit_publisher_receives_filtered_count_after_list_shaping() {
        unimplemented!("mock audit publisher capture per ADR 0015 observability section");
    }
}
