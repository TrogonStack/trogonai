//! Hierarchical policy merge integration tests (ADR 0013).
//!
//! Direct `MergeEngine` evaluation covers merge semantics; gateway audit envelope cases
//! remain ignored until full NATS harness wiring lands.

use std::sync::Arc;

use cel_interpreter::Context;
use trogon_mcp_gateway::authz::{GatewayIdentity, IdentitySource};
use trogon_mcp_gateway::jwt::VerifiedJwtClaims;
use trogon_mcp_gateway::policy::hierarchical::{
    HierarchicalDecision, MergeEngine, MergeRequestContext, PolicyEffect, PolicyLevel, PolicyRule,
    PolicyStore, ScopeBundle, ScopeConfig, ScopeKey, NO_POLICY_CODE,
};
use trogon_mcp_gateway::policy::new_policy_cel_context_for_request;
use trogon_mcp_gateway::rpc_codes;

fn cel_context(method: &str, tool: Option<&str>) -> Context<'static> {
    let identity = GatewayIdentity {
        tenant: Some("acme".into()),
        caller_sub: Some("alice".into()),
        issuer: None,
        jti: None,
        source: IdentitySource::Jwt,
    };
    new_policy_cel_context_for_request(&identity, &VerifiedJwtClaims::default(), &[], method, tool)
        .expect("cel context")
}

fn org_allow_rule() -> PolicyRule {
    PolicyRule {
        policy_id: "org/default-employees".into(),
        revision: 3,
        effect: PolicyEffect::Allow,
        priority: 0,
        cel: r#"jwt.sub == "alice""#.into(),
    }
}

fn tenant_deny_rule() -> PolicyRule {
    PolicyRule {
        policy_id: "tenant/acme-block-secrets".into(),
        revision: 17,
        effect: PolicyEffect::Deny,
        priority: 0,
        cel: r#"mcp.tool.name == "prod_deploy""#.into(),
    }
}

fn seed_org_allow(engine: &MergeEngine) {
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
            rules: vec![org_allow_rule()],
            config: ScopeConfig::default(),
        },
    );
}

mod org_baseline_allow {
    use super::*;

    #[test]
    fn only_org_allow_permits_request() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        seed_org_allow(&engine);

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/list".into(),
            tool_name: None,
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/list", None))
            .expect("eval");
        assert!(matches!(decision, HierarchicalDecision::Allow { .. }));
    }

    #[test]
    fn org_allow_skips_empty_tenant_and_server_layers() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        seed_org_allow(&engine);

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", Some("deploy")))
            .expect("eval");
        assert!(matches!(decision, HierarchicalDecision::Allow { .. }));
    }
}

mod sticky_deny {
    use super::*;

    #[test]
    fn tenant_deny_overrides_org_allow() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        seed_org_allow(&engine);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Tenant,
                tenant: Some("acme".into()),
                server_group: None,
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![tenant_deny_rule()],
                config: ScopeConfig::default(),
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("prod_deploy".into()),
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", Some("prod_deploy")))
            .expect("eval");
        match decision {
            HierarchicalDecision::Deny {
                policy_id,
                code,
                reason,
                ..
            } => {
                assert_eq!(policy_id, Some("tenant/acme-block-secrets".into()));
                assert_eq!(code, rpc_codes::POLICY_DENY);
                assert_eq!(reason, "policy_deny");
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[test]
    fn server_group_deny_overrides_method_allow() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::ServerGroup,
                tenant: Some("acme".into()),
                server_group: Some("incident-response".into()),
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "server-group/no-call".into(),
                    revision: 1,
                    effect: PolicyEffect::Deny,
                    priority: 0,
                    cel: r#"mcp.method == "tools/call""#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Method,
                tenant: Some("acme".into()),
                server_group: Some("incident-response".into()),
                server_id: Some("github".into()),
                method: Some("tools/call".into()),
                tool: Some("deploy".into()),
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "method/allow-deploy".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: "true".into(),
                }],
                config: ScopeConfig::default(),
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: Some("incident-response".into()),
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", Some("deploy")))
            .expect("eval");
        assert!(matches!(
            decision,
            HierarchicalDecision::Deny {
                policy_id: Some(id),
                code: rpc_codes::POLICY_DENY,
                ..
            } if id == "server-group/no-call"
        ));
    }

    #[test]
    fn org_deny_sticks_over_all_lower_layer_allows() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
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
                rules: vec![PolicyRule {
                    policy_id: "org/deny-all".into(),
                    revision: 1,
                    effect: PolicyEffect::Deny,
                    priority: 0,
                    cel: "true".into(),
                }],
                config: ScopeConfig::default(),
            },
        );
        for level in [
            PolicyLevel::Tenant,
            PolicyLevel::ServerGroup,
            PolicyLevel::Server,
            PolicyLevel::Method,
        ] {
            engine.upsert_scope(
                ScopeKey {
                    level,
                    tenant: Some("acme".into()),
                    server_group: Some("proj".into()),
                    server_id: Some("github".into()),
                    method: Some("tools/call".into()),
                    tool: Some("deploy".into()),
                },
                ScopeBundle {
                    rules: vec![PolicyRule {
                        policy_id: format!("{}/allow", level.as_str()),
                        revision: 1,
                        effect: PolicyEffect::Allow,
                        priority: 0,
                        cel: "true".into(),
                    }],
                    config: ScopeConfig::default(),
                },
            );
        }

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: Some("proj".into()),
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", Some("deploy")))
            .expect("eval");
        assert!(matches!(
            decision,
            HierarchicalDecision::Deny {
                policy_id: Some(id),
                code: rpc_codes::POLICY_DENY,
                ..
            } if id == "org/deny-all"
        ));
    }
}

mod default_deny {
    use super::*;

    #[test]
    fn empty_hierarchy_defaults_to_deny() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: None,
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", None))
            .expect("eval");
        assert!(matches!(
            decision,
            HierarchicalDecision::Deny {
                code: NO_POLICY_CODE,
                reason,
                ..
            } if reason == "no_policy"
        ));
    }

    #[test]
    fn no_explicit_allow_side_denies_without_matching_deny() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Tenant,
                tenant: Some("acme".into()),
                server_group: None,
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "tenant/config-only".into(),
                    revision: 1,
                    effect: PolicyEffect::Deny,
                    priority: 0,
                    cel: "false".into(),
                }],
                config: ScopeConfig {
                    rate_max_requests: Some(50),
                    ..ScopeConfig::default()
                },
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: None,
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", None))
            .expect("eval");
        assert!(matches!(
            decision,
            HierarchicalDecision::Deny {
                code: NO_POLICY_CODE,
                ..
            }
        ));
    }
}

mod shallow_cel_merge {
    use super::*;

    #[test]
    fn method_layer_cel_overrides_server_group_allow() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::ServerGroup,
                tenant: Some("acme".into()),
                server_group: Some("proj".into()),
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "server-group/allow-a".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"mcp.tool.name == "other""#.into(),
                }],
                config: ScopeConfig {
                    spicedb_gate_cel: Some(r#"mcp.method == "tools/list""#.into()),
                    ..ScopeConfig::default()
                },
            },
        );
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Method,
                tenant: Some("acme".into()),
                server_group: Some("proj".into()),
                server_id: Some("github".into()),
                method: Some("tools/call".into()),
                tool: Some("deploy".into()),
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "method/allow-b".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"mcp.tool.name == "deploy""#.into(),
                }],
                config: ScopeConfig {
                    spicedb_gate_cel: Some(r#"mcp.method == "tools/call""#.into()),
                    ..ScopeConfig::default()
                },
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: Some("proj".into()),
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
        };
        let effective = engine.effective_policy(&ctx).expect("merge");
        assert_eq!(
            effective.config.spicedb_gate_cel.as_deref(),
            Some(r#"mcp.method == "tools/call""#)
        );
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", Some("deploy")))
            .expect("eval");
        assert!(matches!(decision, HierarchicalDecision::Allow { .. }));
    }

    #[test]
    fn server_group_overrides_tenant_cel_on_shallow_merge() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Tenant,
                tenant: Some("acme".into()),
                server_group: None,
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "tenant/allow".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"mcp.method == "tools/list""#.into(),
                }],
                config: ScopeConfig {
                    spicedb_gate_cel: Some(r#"mcp.method == "tools/list""#.into()),
                    ..ScopeConfig::default()
                },
            },
        );
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::ServerGroup,
                tenant: Some("acme".into()),
                server_group: Some("proj".into()),
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "server-group/allow".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"mcp.method == "tools/call""#.into(),
                }],
                config: ScopeConfig {
                    spicedb_gate_cel: Some(
                        r#"mcp.method == "tools/call" || mcp.method == "resources/read""#.into(),
                    ),
                    ..ScopeConfig::default()
                },
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: Some("proj".into()),
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
        };
        let effective = engine.effective_policy(&ctx).expect("merge");
        assert_eq!(
            effective.config.spicedb_gate_cel.as_deref(),
            Some(r#"mcp.method == "tools/call" || mcp.method == "resources/read""#)
        );
    }

    #[test]
    fn merged_allow_side_or_across_non_conflicting_layers() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        seed_org_allow(&engine);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Tenant,
                tenant: Some("acme".into()),
                server_group: None,
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![PolicyRule {
                    policy_id: "tenant/call-allow".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"mcp.method == "tools/call""#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );

        let list_ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/list".into(),
            tool_name: None,
        };
        let call_ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
        };

        assert!(matches!(
            engine.evaluate(&list_ctx, &cel_context("tools/list", None)).expect("eval"),
            HierarchicalDecision::Allow { .. }
        ));
        assert!(matches!(
            engine
                .evaluate(&call_ctx, &cel_context("tools/call", Some("deploy")))
                .expect("eval"),
            HierarchicalDecision::Allow { .. }
        ));
    }
}

mod audit_rules_fired {
    use super::*;

    #[tokio::test]
    #[ignore = "requires gateway NATS harness for audit envelope publish"]
    async fn rules_fired_lists_org_and_tenant_contributors() {
        unimplemented!("gateway audit envelope wiring pending");
    }

    #[tokio::test]
    #[ignore = "requires gateway NATS harness for audit envelope publish"]
    async fn rules_fired_spans_all_merged_hierarchy_levels() {
        unimplemented!("gateway audit envelope wiring pending");
    }

    #[tokio::test]
    #[ignore = "requires gateway NATS harness for audit envelope publish"]
    async fn audit_envelope_carries_policy_merge_expression_hash() {
        unimplemented!("gateway audit envelope wiring pending");
    }

    #[test]
    fn rules_fired_lists_contributors_at_merge_time() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(store, true);
        seed_org_allow(&engine);
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Tenant,
                tenant: Some("acme".into()),
                server_group: None,
                server_id: None,
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![tenant_deny_rule()],
                config: ScopeConfig::default(),
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("prod_deploy".into()),
        };
        let decision = engine
            .evaluate(&ctx, &cel_context("tools/call", Some("prod_deploy")))
            .expect("eval");
        match decision {
            HierarchicalDecision::Deny { rules_fired, expression_hash, .. } => {
                assert!(rules_fired.contains(&"org/default-employees".to_string()));
                assert!(rules_fired.contains(&"tenant/acme-block-secrets".to_string()));
                assert!(expression_hash.starts_with("sha256:"));
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }
}
