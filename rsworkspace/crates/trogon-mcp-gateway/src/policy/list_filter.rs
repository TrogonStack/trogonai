//! Per-item CEL re-evaluation for `tools/list` catalog shaping (ADR 0015).

use std::sync::Arc;

use cel_interpreter::objects::Key;
use cel_interpreter::Context;
use tracing::warn;

use crate::act_chain::ActChainEntry;
use crate::authz::{GatewayIdentity, PermissionChecker};
use crate::cel_builtins::{classify_list_filter_host_error, HostEvalContext, HostFailure, PermissionCheckerSpicedbBackend};
use crate::jwt::VerifiedJwtClaims;
use crate::policy::hierarchical::{
    self, CompiledRule, EffectivePolicy, HierarchicalDecision,     MergeEngine, MergeError, MergeRequestContext,
};
use crate::policy::{configure_policy_cel_context, evaluate_cel_with_host_classified, CelHostEvalOutcome, PolicyError};

pub const AUDIT_FILTERED_BY_CEL: &str = "filtered_by_cel";
pub const AUDIT_CEL_EVAL_ERROR: &str = "cel_eval_error";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListFilterAuditEvent {
    pub event: &'static str,
    pub tool_name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCandidate {
    pub name: String,
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListFilterOutcome {
    pub kept: Vec<ToolCandidate>,
    pub audit_events: Vec<ListFilterAuditEvent>,
}

pub struct ListFilterParams<'a> {
    pub identity: &'a GatewayIdentity,
    pub claims: &'a VerifiedJwtClaims,
    pub act_chain: &'a [ActChainEntry],
    pub tenant: &'a str,
    pub server_id: &'a str,
    pub session_id: &'a str,
    pub checker: Arc<dyn PermissionChecker>,
    pub candidates: Vec<ToolCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ToolDisposition {
    Keep,
    DropFiltered { reason: &'static str },
    DropPermanent { reason: String },
}

/// Apply hierarchical CEL policy per tool after BulkCheck filtering.
#[must_use]
pub fn filter_tools_by_cel(params: ListFilterParams<'_>) -> ListFilterOutcome {
    let Some(engine) = hierarchical::shared() else {
        return passthrough_outcome(params.candidates);
    };
    filter_tools_by_cel_with_engine(engine.as_ref(), params)
}

/// Same as [`filter_tools_by_cel`] with an explicit merge engine (unit tests).
#[must_use]
pub fn filter_tools_by_cel_with_engine(engine: &MergeEngine, params: ListFilterParams<'_>) -> ListFilterOutcome {
    let bundle_revision = sample_bundle_revision(engine, params.tenant, params.server_id);
    let host = build_host_eval_context(
        params.identity,
        params.tenant,
        params.server_id,
        params.session_id,
        params.checker,
        bundle_revision,
    );

    let mut kept = Vec::new();
    let mut audit_events = Vec::new();

    for (index, tool) in params.candidates.into_iter().enumerate() {
        let merge_ctx = MergeRequestContext {
            tenant: params.tenant.to_string(),
            server_group: None,
            server_id: params.server_id.to_string(),
            jsonrpc_method: "tools/list".into(),
            tool_name: Some(tool.name.clone()),
        };

        let disposition = match engine.effective_policy(&merge_ctx) {
            Ok(effective) => evaluate_tool_visibility(
                &effective,
                &host,
                params.identity,
                params.claims,
                params.act_chain,
                index,
                &tool,
            ),
            Err(MergeError::CelEval(_)) => ToolDisposition::DropFiltered {
                reason: "cel_runtime_error",
            },
            Err(MergeError::CelCompile { detail, .. }) => ToolDisposition::DropPermanent { reason: detail },
        };

        match disposition {
            ToolDisposition::Keep => kept.push(tool),
            ToolDisposition::DropFiltered { reason } => {
                audit_events.push(ListFilterAuditEvent {
                    event: AUDIT_FILTERED_BY_CEL,
                    tool_name: tool.name,
                    reason: reason.to_string(),
                });
            }
            ToolDisposition::DropPermanent { reason } => {
                audit_events.push(ListFilterAuditEvent {
                    event: AUDIT_CEL_EVAL_ERROR,
                    tool_name: tool.name.clone(),
                    reason: reason.clone(),
                });
                audit_events.push(ListFilterAuditEvent {
                    event: AUDIT_FILTERED_BY_CEL,
                    tool_name: tool.name,
                    reason,
                });
            }
        }
    }

    ListFilterOutcome { kept, audit_events }
}

fn passthrough_outcome(candidates: Vec<ToolCandidate>) -> ListFilterOutcome {
    ListFilterOutcome {
        kept: candidates,
        audit_events: Vec::new(),
    }
}

fn sample_bundle_revision(engine: &MergeEngine, tenant: &str, server_id: &str) -> String {
    let ctx = MergeRequestContext {
        tenant: tenant.to_string(),
        server_group: None,
        server_id: server_id.to_string(),
        jsonrpc_method: "tools/list".into(),
        tool_name: None,
    };
    engine
        .effective_policy(&ctx)
        .map(|effective| effective.expression_hash.clone())
        .unwrap_or_else(|_| "rev-0".into())
}

fn build_host_eval_context(
    identity: &GatewayIdentity,
    tenant: &str,
    server_id: &str,
    session_id: &str,
    checker: Arc<dyn PermissionChecker>,
    bundle_revision: String,
) -> HostEvalContext {
    let backend = PermissionCheckerSpicedbBackend::new(
        checker,
        identity.tenant.clone(),
        identity.caller_sub.clone(),
        Some(session_id.to_string()),
        server_id.to_string(),
    );
    let mut host = HostEvalContext::for_tests().with_spicedb(Arc::new(backend));
    host.tenant_id = tenant.to_string();
    host.bundle_revision = bundle_revision;
    host.session_id = Some(session_id.to_string());
    host.server_id = Some(server_id.to_string());
    host
}

fn evaluate_tool_visibility(
    effective: &EffectivePolicy,
    host: &HostEvalContext,
    identity: &GatewayIdentity,
    claims: &VerifiedJwtClaims,
    act_chain: &[ActChainEntry],
    list_filter_index: usize,
    tool: &ToolCandidate,
) -> ToolDisposition {
    let configure = |ctx: &mut Context| {
        configure_list_filter_context(ctx, identity, claims, act_chain, list_filter_index, tool)
    };

    match evaluate_effective_with_host(effective, host, configure) {
        Ok(HierarchicalDecision::Allow { .. }) => ToolDisposition::Keep,
        Ok(HierarchicalDecision::Deny { reason, .. }) => ToolDisposition::DropFiltered {
            reason: match reason.as_str() {
                "policy_deny" => "policy_deny",
                "no_policy" => "no_policy",
                _ => "policy_deny",
            },
        },
        Err(outcome) => outcome,
    }
}

fn configure_list_filter_context(
    ctx: &mut Context,
    identity: &GatewayIdentity,
    claims: &VerifiedJwtClaims,
    act_chain: &[ActChainEntry],
    list_filter_index: usize,
    tool: &ToolCandidate,
) -> Result<(), PolicyError> {
    configure_policy_cel_context(ctx, identity, claims, act_chain)?;

    let mcp = if let Some(schema) = tool.input_schema.as_ref() {
        serde_json::json!({
            "method": "tools/list",
            "tool": { "name": tool.name, "inputSchema": schema }
        })
    } else {
        serde_json::json!({
            "method": "tools/list",
            "tool": { "name": tool.name }
        })
    };
    let mcp_value = cel_interpreter::to_value(&mcp).map_err(|e| PolicyError(e.to_string()))?;
    ctx.add_variable_from_value("mcp", mcp_value);

    let response = response_cel_object(list_filter_index);
    ctx.add_variable_from_value("response", response);
    Ok(())
}

fn response_cel_object(list_filter_index: usize) -> cel_interpreter::Value {
    use std::collections::HashMap;

    let mut map = HashMap::new();
    map.insert(
        Key::from("list_filter_index"),
        cel_interpreter::Value::Int(list_filter_index as i64),
    );
    cel_interpreter::Value::Map(map.into())
}

fn evaluate_effective_with_host(
    effective: &EffectivePolicy,
    host: &HostEvalContext,
    configure: impl Fn(&mut Context) -> Result<(), PolicyError>,
) -> Result<HierarchicalDecision, ToolDisposition> {
    if effective.allow_rules.is_empty() && effective.deny_rules.is_empty() {
        return Ok(HierarchicalDecision::Deny {
            reason: "no_policy".into(),
            policy_id: None,
            code: hierarchical::NO_POLICY_CODE,
            rules_fired: Vec::new(),
            expression_hash: effective.expression_hash.clone(),
        });
    }

    for rule in &effective.deny_rules {
        match eval_rule_with_host(rule, host, &configure) {
            Ok(true) => {
                return Ok(HierarchicalDecision::Deny {
                    reason: "policy_deny".into(),
                    policy_id: Some(rule.contributor.policy_id.clone()),
                    code: crate::rpc_codes::POLICY_DENY,
                    rules_fired: Vec::new(),
                    expression_hash: effective.expression_hash.clone(),
                });
            }
            Ok(false) => {}
            Err(disposition) => return Err(disposition),
        }
    }

    for rule in &effective.allow_rules {
        match eval_rule_with_host(rule, host, &configure) {
            Ok(true) => {
                return Ok(HierarchicalDecision::Allow {
                    rules_fired: Vec::new(),
                    expression_hash: effective.expression_hash.clone(),
                });
            }
            Ok(false) => {}
            Err(disposition) => return Err(disposition),
        }
    }

    Ok(HierarchicalDecision::Deny {
        reason: "no_policy".into(),
        policy_id: None,
        code: hierarchical::NO_POLICY_CODE,
        rules_fired: Vec::new(),
        expression_hash: effective.expression_hash.clone(),
    })
}

fn eval_rule_with_host(
    rule: &CompiledRule,
    host: &HostEvalContext,
    configure: &impl Fn(&mut Context) -> Result<(), PolicyError>,
) -> Result<bool, ToolDisposition> {
    match evaluate_cel_with_host_classified(&rule.program, host, |ctx| configure(ctx)) {
        CelHostEvalOutcome::Bool(value) => Ok(value),
        CelHostEvalOutcome::HostFailure(failure, detail) => Err(match failure {
            HostFailure::Transient => ToolDisposition::DropFiltered {
                reason: "authz_unreachable",
            },
            HostFailure::Permanent => ToolDisposition::DropPermanent { reason: detail },
            HostFailure::NotApplicable => ToolDisposition::Keep,
        }),
        CelHostEvalOutcome::Runtime(detail) => {
            if let Some(failure) = classify_list_filter_host_error(&detail) {
                Err(match failure {
                    HostFailure::Transient => ToolDisposition::DropFiltered {
                        reason: "authz_unreachable",
                    },
                    HostFailure::Permanent => ToolDisposition::DropPermanent {
                        reason: detail.clone(),
                    },
                    HostFailure::NotApplicable => ToolDisposition::Keep,
                })
            } else {
                warn!(
                    policy_id = %rule.contributor.policy_id,
                    effect = ?rule.effect,
                    error = %detail,
                    "tools/list per-tool CEL runtime error"
                );
                Err(ToolDisposition::DropFiltered {
                    reason: "cel_runtime_error",
                })
            }
        }
    }
}

/// Emit list-filter audit events without logging tool schema payloads.
pub fn log_list_filter_audit_events(server_id: &str, events: &[ListFilterAuditEvent]) {
    for event in events {
        warn!(
            audit.event = event.event,
            tool.name = %event.tool_name,
            server_id = %server_id,
            reason = %event.reason,
            "tools/list CEL catalog filter"
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::authz::{AllowAllPermissionChecker, AuthzContext, AuthzError, IdentitySource};
    use crate::policy::hierarchical::{
        MergeEngine, PolicyEffect, PolicyLevel, PolicyRule, PolicyStore, ScopeBundle, ScopeConfig, ScopeKey,
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

    fn seed_allow_by_sub(engine: &MergeEngine) {
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
                    policy_id: "org/default".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"jwt.sub == "alice""#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );
    }

    fn seed_deny_by_tool(engine: &MergeEngine, tool: &str) {
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
                    policy_id: "tenant/deny-tool".into(),
                    revision: 1,
                    effect: PolicyEffect::Deny,
                    priority: 0,
                    cel: format!(r#"mcp.tool.name == "{tool}""#),
                }],
                config: ScopeConfig::default(),
            },
        );
    }

    fn install_engine(_engine: Arc<MergeEngine>) {
        // Production installs via `hierarchical::set_shared`; unit tests pass the engine explicitly.
    }

    fn run_filter(engine: &MergeEngine, candidates: Vec<ToolCandidate>) -> ListFilterOutcome {
        let identity = sample_identity();
        filter_tools_by_cel_with_engine(
            engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-1",
                checker: Arc::new(AllowAllPermissionChecker),
                candidates,
            },
        )
    }

    #[test]
    fn transient_host_failure_drops_tool_with_deny_bias() {
        struct FailChecker;

        #[async_trait::async_trait]
        impl PermissionChecker for FailChecker {
            async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
                Err(AuthzError("pdp down".into()))
            }
        }

        let store = Arc::new(PolicyStore::default());
        let engine = Arc::new(MergeEngine::new(Arc::clone(&store), true));
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
                    policy_id: "org/spicedb-gate".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"spicedb.check("user:alice", "invoke", "tool:github|deploy")"#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );

        let identity = sample_identity();
        let outcome = filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-1",
                checker: Arc::new(FailChecker),
                candidates: vec![ToolCandidate {
                    name: "deploy".into(),
                    input_schema: None,
                }],
            },
        );

        assert!(outcome.kept.is_empty());
        assert_eq!(outcome.audit_events.len(), 1);
        assert_eq!(outcome.audit_events[0].event, AUDIT_FILTERED_BY_CEL);
        assert_eq!(outcome.audit_events[0].reason, "authz_unreachable");
    }

    #[test]
    fn permanent_host_failure_emits_cel_eval_error_audit() {
        let store = Arc::new(PolicyStore::default());
        let engine = Arc::new(MergeEngine::new(Arc::clone(&store), true));
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
                    policy_id: "org/spicedb-gate".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"spicedb.check("user:alice", "invoke", "not-a-tool-ref")"#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );

        let identity = sample_identity();
        let outcome = filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-1",
                checker: Arc::new(AllowAllPermissionChecker),
                candidates: vec![ToolCandidate {
                    name: "deploy".into(),
                    input_schema: None,
                }],
            },
        );

        assert!(outcome.kept.is_empty());
        assert!(
            outcome
                .audit_events
                .iter()
                .any(|event| event.event == AUDIT_CEL_EVAL_ERROR)
        );
        assert!(
            outcome
                .audit_events
                .iter()
                .any(|event| event.event == AUDIT_FILTERED_BY_CEL)
        );

        let _ = hierarchical::set_shared(engine);
    }

    #[test]
    fn not_applicable_without_shared_engine_keeps_bulk_checked_tools() {
        let identity = sample_identity();
        let outcome = filter_tools_by_cel(ListFilterParams {
            identity: &identity,
            claims: &VerifiedJwtClaims::default(),
            act_chain: &[],
            tenant: "acme",
            server_id: "github",
            session_id: "sess-1",
            checker: Arc::new(AllowAllPermissionChecker),
            candidates: vec![
                ToolCandidate {
                    name: "a".into(),
                    input_schema: None,
                },
                ToolCandidate {
                    name: "b".into(),
                    input_schema: None,
                },
            ],
        });

        assert_eq!(outcome.kept.len(), 2);
        assert!(outcome.audit_events.is_empty());
    }

    #[test]
    fn policy_deny_emits_filtered_by_cel_audit() {
        let store = Arc::new(PolicyStore::default());
        let engine = Arc::new(MergeEngine::new(Arc::clone(&store), true));
        seed_allow_by_sub(&engine);
        seed_deny_by_tool(&engine, "secret");
        install_engine(Arc::clone(&engine));

        let outcome = run_filter(
            &engine,
            vec![
                ToolCandidate {
                    name: "read_file".into(),
                    input_schema: None,
                },
                ToolCandidate {
                    name: "secret".into(),
                    input_schema: None,
                },
            ],
        );

        assert_eq!(outcome.kept.len(), 1);
        assert_eq!(outcome.kept[0].name, "read_file");
        assert_eq!(outcome.audit_events.len(), 1);
        assert_eq!(outcome.audit_events[0].event, AUDIT_FILTERED_BY_CEL);
        assert_eq!(outcome.audit_events[0].tool_name, "secret");
    }

    #[test]
    fn response_list_filter_index_is_visible_to_cel() {
        let store = Arc::new(PolicyStore::default());
        let engine = Arc::new(MergeEngine::new(Arc::clone(&store), true));
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
                    policy_id: "org/even-index".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: "response.list_filter_index % 2 == 0".into(),
                }],
                config: ScopeConfig::default(),
            },
        );
        install_engine(Arc::clone(&engine));

        let outcome = run_filter(
            &engine,
            vec![
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
        );

        assert_eq!(
            outcome.kept.iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            vec!["t0", "t2"]
        );
    }

    #[test]
    fn zedtoken_cache_reused_across_per_tool_evals() {
        let store = Arc::new(PolicyStore::default());
        let engine = Arc::new(MergeEngine::new(Arc::clone(&store), true));
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
                    policy_id: "org/spicedb-all".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"spicedb.check("user:alice", "invoke", "tool:github|probe")"#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );

        let cache = Arc::new(crate::spicedb::ZedTokenCache::default());
        let params = crate::spicedb::CacheKeyParams {
            session_id: "sess-zed".into(),
            principal: "trogon/principal|alice".into(),
            permission: "invoke".into(),
        };
        futures::executor::block_on(async {
            cache
                .insert_zed_token(&params, "zed-session-token".into(), None)
                .await;
        });

        let zed_reads = Arc::new(AtomicUsize::new(0));
        struct ZedTrackingChecker {
            cache: Arc<crate::spicedb::ZedTokenCache>,
            params: crate::spicedb::CacheKeyParams,
            reads: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl PermissionChecker for ZedTrackingChecker {
            async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
                if self.cache.get_zed_token(&self.params).await.is_some() {
                    self.reads.fetch_add(1, Ordering::SeqCst);
                }
                Ok(true)
            }
        }

        let checker = Arc::new(ZedTrackingChecker {
            cache: Arc::clone(&cache),
            params: params.clone(),
            reads: Arc::clone(&zed_reads),
        });
        let identity = sample_identity();
        let outcome = filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-zed",
                checker,
                candidates: vec![
                    ToolCandidate {
                        name: "alpha".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "beta".into(),
                        input_schema: None,
                    },
                ],
            },
        );

        assert_eq!(outcome.kept.len(), 2);
        assert_eq!(zed_reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            futures::executor::block_on(async { cache.get_zed_token(&params).await }),
            Some("zed-session-token".into())
        );
    }

    struct CountingChecker {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl PermissionChecker for CountingChecker {
        async fn authorize_mcp_request(&self, _ctx: AuthzContext<'_>) -> Result<bool, AuthzError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }

    #[test]
    fn shared_host_context_reuses_checker_backend_across_tools() {
        let store = Arc::new(PolicyStore::default());
        let engine = Arc::new(MergeEngine::new(Arc::clone(&store), true));
        seed_allow_by_sub(&engine);
        install_engine(Arc::clone(&engine));

        let calls = Arc::new(AtomicUsize::new(0));
        let checker = Arc::new(CountingChecker {
            calls: Arc::clone(&calls),
        });
        let identity = sample_identity();

        let _ = filter_tools_by_cel_with_engine(
            &engine,
            ListFilterParams {
                identity: &identity,
                claims: &VerifiedJwtClaims::default(),
                act_chain: &[],
                tenant: "acme",
                server_id: "github",
                session_id: "sess-shared",
                checker,
                candidates: vec![
                    ToolCandidate {
                        name: "a".into(),
                        input_schema: None,
                    },
                    ToolCandidate {
                        name: "b".into(),
                        input_schema: None,
                    },
                ],
            },
        );

        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
