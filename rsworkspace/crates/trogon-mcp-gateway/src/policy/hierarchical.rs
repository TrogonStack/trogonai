//! Hierarchical policy merge per ADR 0013: org → tenant → server-group → server → method.
//!
//! Authorization: `ALLOW ⇔ (A_org ∨ … ∨ A_method) ∧ ¬(D_org ∨ … ∨ D_method)`.
//! Config overlays: scalar overwrite (more-specific wins), list concat, rate cap narrow-only (min).

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use cel_interpreter::{Context, Program, Value};
use sha2::{Digest, Sha256};

use crate::rpc_codes;

pub const MERGE_MODEL: &str = "hierarchical/v1";
pub const NO_POLICY_CODE: i32 = -32_108;
pub const ENV_MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE: &str = "MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HierarchicalPolicyConfig {
    pub enforce: bool,
}

pub fn hierarchical_policy_config<E: trogon_std::env::ReadEnv>(env: &E) -> HierarchicalPolicyConfig {
    let enforce = env
        .var(ENV_MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE)
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);
    HierarchicalPolicyConfig { enforce }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum PolicyLevel {
    Org = 1,
    Tenant = 2,
    ServerGroup = 3,
    Server = 4,
    Method = 5,
}

impl PolicyLevel {
    pub const ORDER: [Self; 5] = [
        Self::Org,
        Self::Tenant,
        Self::ServerGroup,
        Self::Server,
        Self::Method,
    ];

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Tenant => "tenant",
            Self::ServerGroup => "server-group",
            Self::Server => "server",
            Self::Method => "method",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyEffect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyRule {
    pub policy_id: String,
    pub revision: u64,
    pub effect: PolicyEffect,
    pub priority: i32,
    pub cel: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScopeConfig {
    pub spicedb_gate_cel: Option<String>,
    pub rate_max_requests: Option<u32>,
    pub redaction_paths: Vec<String>,
    pub audit_tags: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScopeBundle {
    pub rules: Vec<PolicyRule>,
    pub config: ScopeConfig,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ScopeKey {
    pub level: PolicyLevel,
    pub tenant: Option<String>,
    pub server_group: Option<String>,
    pub server_id: Option<String>,
    pub method: Option<String>,
    pub tool: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MergeRequestContext {
    pub tenant: String,
    pub server_group: Option<String>,
    pub server_id: String,
    pub jsonrpc_method: String,
    pub tool_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolicyContributor {
    pub policy_id: String,
    pub revision: u64,
    pub level: PolicyLevel,
}

#[derive(Debug)]
pub struct CompiledRule {
    pub contributor: PolicyContributor,
    pub effect: PolicyEffect,
    pub program: Program,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MergedConfig {
    pub spicedb_gate_cel: Option<String>,
    pub rate_max_requests: Option<u32>,
    pub redaction_paths: Vec<String>,
    pub audit_tags: Vec<String>,
}

#[derive(Debug)]
pub struct EffectivePolicy {
    pub allow_rules: Vec<CompiledRule>,
    pub deny_rules: Vec<CompiledRule>,
    pub config: MergedConfig,
    pub contributors: Vec<PolicyContributor>,
    pub expression_hash: String,
    pub merge_model: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HierarchicalDecision {
    Allow {
        rules_fired: Vec<String>,
        expression_hash: String,
    },
    Deny {
        reason: String,
        policy_id: Option<String>,
        code: i32,
        rules_fired: Vec<String>,
        expression_hash: String,
    },
}

#[derive(Debug)]
pub enum MergeError {
    CelCompile { policy_id: String, detail: String },
    CelEval(String),
}

impl fmt::Display for MergeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CelCompile { policy_id, detail } => {
                write!(f, "CEL compile failed for {policy_id}: {detail}")
            }
            Self::CelEval(detail) => write!(f, "CEL evaluation failed: {detail}"),
        }
    }
}

impl std::error::Error for MergeError {}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    tenant: String,
    server_id: String,
    tool: Option<String>,
    generation: u64,
}

#[derive(Default)]
struct PolicyStoreInner {
    scopes: HashMap<ScopeKey, ScopeBundle>,
}

#[derive(Default)]
pub struct PolicyStore {
    inner: RwLock<PolicyStoreInner>,
}

impl PolicyStore {
    pub fn upsert(&self, key: ScopeKey, bundle: ScopeBundle) {
        let mut guard = self.inner.write().expect("policy store lock");
        guard.scopes.insert(key, bundle);
    }

    pub fn remove(&self, key: &ScopeKey) {
        let mut guard = self.inner.write().expect("policy store lock");
        guard.scopes.remove(key);
    }

    pub fn bundle(&self, key: &ScopeKey) -> Option<ScopeBundle> {
        self.inner.read().expect("policy store lock").scopes.get(key).cloned()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.read().expect("policy store lock").scopes.is_empty()
    }
}

pub struct MergeEngine {
    store: Arc<PolicyStore>,
    cache: RwLock<HashMap<CacheKey, Arc<EffectivePolicy>>>,
    generation: AtomicU64,
    enforce: bool,
}

static SHARED_ENGINE: OnceLock<Arc<MergeEngine>> = OnceLock::new();

/// Install the process-wide merge engine for gateway hot-path evaluation.
pub fn set_shared(engine: Arc<MergeEngine>) -> Result<(), Arc<MergeEngine>> {
    SHARED_ENGINE.set(engine)
}

/// Returns the installed merge engine, if any.
#[must_use]
pub fn shared() -> Option<Arc<MergeEngine>> {
    SHARED_ENGINE.get().cloned()
}

/// Returns a process-wide engine when `MCP_GATEWAY_HIERARCHICAL_POLICY_ENFORCE` is set.
pub fn maybe_install_from_env<E: trogon_std::env::ReadEnv>(env: &E) -> Option<Arc<MergeEngine>> {
    let cfg = hierarchical_policy_config(env);
    if !cfg.enforce {
        return None;
    }
    let engine = Arc::new(MergeEngine::new(Arc::new(PolicyStore::default()), true));
    let _ = set_shared(Arc::clone(&engine));
    Some(engine)
}

impl MergeEngine {
    #[must_use]
    pub fn new(store: Arc<PolicyStore>, enforce: bool) -> Self {
        Self {
            store,
            cache: RwLock::new(HashMap::new()),
            generation: AtomicU64::new(0),
            enforce,
        }
    }

    pub fn upsert_scope(&self, key: ScopeKey, bundle: ScopeBundle) {
        self.store.upsert(key, bundle);
        self.bump_generation();
    }

    pub fn remove_scope(&self, key: &ScopeKey) {
        self.store.remove(key);
        self.bump_generation();
    }

    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.cache.write().expect("policy cache lock").clear();
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    pub fn effective_policy(&self, ctx: &MergeRequestContext) -> Result<Arc<EffectivePolicy>, MergeError> {
        let generation = self.generation();
        let cache_key = CacheKey {
            tenant: ctx.tenant.clone(),
            server_id: ctx.server_id.clone(),
            tool: ctx.tool_name.clone(),
            generation,
        };

        if let Some(cached) = self.cache.read().expect("policy cache lock").get(&cache_key) {
            return Ok(Arc::clone(cached));
        }

        let merged = Arc::new(merge_scopes(&self.store, ctx)?);
        self.cache
            .write()
            .expect("policy cache lock")
            .insert(cache_key, Arc::clone(&merged));
        Ok(merged)
    }

    pub fn evaluate(
        &self,
        ctx: &MergeRequestContext,
        cel_ctx: &Context,
    ) -> Result<HierarchicalDecision, MergeError> {
        if !self.enforce && self.store.is_empty() {
            return Ok(HierarchicalDecision::Allow {
                rules_fired: Vec::new(),
                expression_hash: String::new(),
            });
        }

        let effective = self.effective_policy(ctx)?;
        evaluate_effective(&effective, cel_ctx)
    }
}

fn scope_keys_for(ctx: &MergeRequestContext) -> [ScopeKey; 5] {
    [
        ScopeKey {
            level: PolicyLevel::Org,
            tenant: None,
            server_group: None,
            server_id: None,
            method: None,
            tool: None,
        },
        ScopeKey {
            level: PolicyLevel::Tenant,
            tenant: Some(ctx.tenant.clone()),
            server_group: None,
            server_id: None,
            method: None,
            tool: None,
        },
        ScopeKey {
            level: PolicyLevel::ServerGroup,
            tenant: Some(ctx.tenant.clone()),
            server_group: ctx.server_group.clone(),
            server_id: None,
            method: None,
            tool: None,
        },
        ScopeKey {
            level: PolicyLevel::Server,
            tenant: Some(ctx.tenant.clone()),
            server_group: ctx.server_group.clone(),
            server_id: Some(ctx.server_id.clone()),
            method: None,
            tool: None,
        },
        ScopeKey {
            level: PolicyLevel::Method,
            tenant: Some(ctx.tenant.clone()),
            server_group: ctx.server_group.clone(),
            server_id: Some(ctx.server_id.clone()),
            method: Some(ctx.jsonrpc_method.clone()),
            tool: ctx.tool_name.clone(),
        },
    ]
}

fn merge_scopes(store: &PolicyStore, ctx: &MergeRequestContext) -> Result<EffectivePolicy, MergeError> {
    let keys = scope_keys_for(ctx);
    let mut allow_rules = Vec::new();
    let mut deny_rules = Vec::new();
    let mut contributors = Vec::new();
    let mut config = MergedConfig::default();
    let mut any_bundle = false;

    for key in &keys {
        let Some(bundle) = store.bundle(key) else {
            continue;
        };
        any_bundle = true;
        merge_config(&mut config, &bundle.config);
        let level_rules = compile_level_rules(key.level, &bundle.rules)?;
        for rule in level_rules {
            contributors.push(rule.contributor.clone());
            match rule.effect {
                PolicyEffect::Allow => allow_rules.push(rule),
                PolicyEffect::Deny => deny_rules.push(rule),
            }
        }
    }

    if !any_bundle && store.is_empty() {
        // No bundles at any level in the store.
    }

    let expression_hash = canonical_expression_hash(&allow_rules, &deny_rules, &config);

    Ok(EffectivePolicy {
        allow_rules,
        deny_rules,
        config,
        contributors,
        expression_hash,
        merge_model: MERGE_MODEL,
    })
}

fn merge_config(target: &mut MergedConfig, overlay: &ScopeConfig) {
    if let Some(expr) = overlay.spicedb_gate_cel.as_ref() {
        target.spicedb_gate_cel = Some(expr.clone());
    }
    if let Some(rate) = overlay.rate_max_requests {
        target.rate_max_requests = Some(match target.rate_max_requests {
            Some(existing) => existing.min(rate),
            None => rate,
        });
    }
    for path in &overlay.redaction_paths {
        if !target.redaction_paths.iter().any(|p| p == path) {
            target.redaction_paths.push(path.clone());
        }
    }
    for tag in &overlay.audit_tags {
        if !target.audit_tags.iter().any(|t| t == tag) {
            target.audit_tags.push(tag.clone());
        }
    }
}

fn compile_level_rules(level: PolicyLevel, rules: &[PolicyRule]) -> Result<Vec<CompiledRule>, MergeError> {
    let mut sorted = rules.to_vec();
    sorted.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.policy_id.cmp(&b.policy_id))
    });

    let mut compiled = Vec::new();
    for rule in sorted {
        let program = Program::compile(&rule.cel).map_err(|e| MergeError::CelCompile {
            policy_id: rule.policy_id.clone(),
            detail: e.to_string(),
        })?;
        compiled.push(CompiledRule {
            contributor: PolicyContributor {
                policy_id: rule.policy_id.clone(),
                revision: rule.revision,
                level,
            },
            effect: rule.effect,
            program,
        });
    }
    Ok(compiled)
}

fn canonical_expression_hash(
    allow_rules: &[CompiledRule],
    deny_rules: &[CompiledRule],
    config: &MergedConfig,
) -> String {
    let mut canonical = String::new();
    canonical.push_str("allow:");
    for rule in allow_rules {
        canonical.push_str(&rule.contributor.policy_id);
        canonical.push(':');
        canonical.push_str(&rule.contributor.level.as_str());
        canonical.push('|');
    }
    canonical.push_str(";deny:");
    for rule in deny_rules {
        canonical.push_str(&rule.contributor.policy_id);
        canonical.push(':');
        canonical.push_str(&rule.contributor.level.as_str());
        canonical.push('|');
    }
    canonical.push_str(";config:");
    if let Some(expr) = config.spicedb_gate_cel.as_ref() {
        canonical.push_str(expr);
    }
    if let Some(rate) = config.rate_max_requests {
        canonical.push_str(&format!(";rate={rate}"));
    }
    for path in &config.redaction_paths {
        canonical.push_str(";redact=");
        canonical.push_str(path);
    }

    let digest = Sha256::digest(canonical.as_bytes());
    format!("sha256:{}", hex::encode(digest))
}

fn evaluate_rule(rule: &CompiledRule, cel_ctx: &Context) -> Result<bool, MergeError> {
    match rule.program.execute(cel_ctx) {
        Ok(Value::Bool(b)) => Ok(b),
        Ok(other) => Err(MergeError::CelEval(format!(
            "rule {} must yield bool, got {:?}",
            rule.contributor.policy_id,
            other.type_of()
        ))),
        Err(e) => Err(MergeError::CelEval(e.to_string())),
    }
}

fn rules_fired_ids(effective: &EffectivePolicy) -> Vec<String> {
    effective
        .contributors
        .iter()
        .map(|c| c.policy_id.clone())
        .collect()
}

fn evaluate_effective(
    effective: &EffectivePolicy,
    cel_ctx: &Context,
) -> Result<HierarchicalDecision, MergeError> {
    let rules_fired = rules_fired_ids(effective);
    let expression_hash = effective.expression_hash.clone();

    if effective.allow_rules.is_empty() && effective.deny_rules.is_empty() {
        return Ok(HierarchicalDecision::Deny {
            reason: "no_policy".into(),
            policy_id: None,
            code: NO_POLICY_CODE,
            rules_fired,
            expression_hash,
        });
    }

    let mut matching_deny: Option<&CompiledRule> = None;
    for rule in &effective.deny_rules {
        if evaluate_rule(rule, cel_ctx)? {
            matching_deny = Some(rule);
            break;
        }
    }

    if let Some(deny_rule) = matching_deny {
        return Ok(HierarchicalDecision::Deny {
            reason: "policy_deny".into(),
            policy_id: Some(deny_rule.contributor.policy_id.clone()),
            code: rpc_codes::POLICY_DENY,
            rules_fired,
            expression_hash,
        });
    }

    let mut allow_matched = false;
    for rule in &effective.allow_rules {
        if evaluate_rule(rule, cel_ctx)? {
            allow_matched = true;
            break;
        }
    }

    if !allow_matched {
        return Ok(HierarchicalDecision::Deny {
            reason: "no_policy".into(),
            policy_id: None,
            code: NO_POLICY_CODE,
            rules_fired,
            expression_hash,
        });
    }

    Ok(HierarchicalDecision::Allow {
        rules_fired,
        expression_hash,
    })
}

/// Same-level deny wins over allow when both match (deny-bias at evaluation time).
pub fn same_level_deny_wins(rules: &[PolicyRule], matches: &[(usize, bool)]) -> PolicyEffect {
    let mut has_allow = false;
    for (idx, matched) in matches {
        if !*matched {
            continue;
        }
        match rules[*idx].effect {
            PolicyEffect::Deny => return PolicyEffect::Deny,
            PolicyEffect::Allow => has_allow = true,
        }
    }
    if has_allow {
        PolicyEffect::Allow
    } else {
        PolicyEffect::Deny
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::IdentitySource;
    use crate::authz::GatewayIdentity;
    use crate::jwt::VerifiedJwtClaims;
    use crate::policy::configure_policy_cel_context;

    fn cel_context(method: &str, tool: Option<&str>) -> Context<'static> {
        let mut ctx = Context::default();
        let mcp = if let Some(tool) = tool {
            serde_json::json!({ "method": method, "tool": { "name": tool } })
        } else {
            serde_json::json!({ "method": method })
        };
        let value = cel_interpreter::to_value(&mcp).unwrap();
        ctx.add_variable_from_value("mcp", value);

        let identity = GatewayIdentity {
            tenant: Some("acme".into()),
            caller_sub: Some("alice".into()),
            issuer: None,
            jti: None,
            source: IdentitySource::Jwt,
        };
        let claims = VerifiedJwtClaims::default();
        configure_policy_cel_context(&mut ctx, &identity, &claims, &[]).unwrap();
        ctx
    }

    fn org_allow_rule() -> PolicyRule {
        PolicyRule {
            policy_id: "org/default-employees".into(),
            revision: 1,
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

    #[test]
    fn deny_bias_same_level() {
        let rules = vec![
            PolicyRule {
                policy_id: "a".into(),
                revision: 1,
                effect: PolicyEffect::Allow,
                priority: 10,
                cel: "true".into(),
            },
            PolicyRule {
                policy_id: "b".into(),
                revision: 1,
                effect: PolicyEffect::Deny,
                priority: 0,
                cel: "true".into(),
            },
        ];
        assert_eq!(
            same_level_deny_wins(&rules, &[(0, true), (1, true)]),
            PolicyEffect::Deny
        );
    }

    #[test]
    fn scope_precedence_tenant_deny_overrides_org_allow() {
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
                rules: vec![org_allow_rule()],
                config: ScopeConfig::default(),
            },
        );
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

        let cel = cel_context("tools/call", Some("prod_deploy"));
        let decision = engine.evaluate(&ctx, &cel).unwrap();
        match decision {
            HierarchicalDecision::Deny {
                reason,
                policy_id,
                code,
                rules_fired,
                expression_hash,
            } => {
                assert_eq!(reason, "policy_deny");
                assert_eq!(policy_id, Some("tenant/acme-block-secrets".into()));
                assert_eq!(code, rpc_codes::POLICY_DENY);
                assert_eq!(
                    rules_fired,
                    vec![
                        "org/default-employees".to_string(),
                        "tenant/acme-block-secrets".to_string(),
                    ]
                );
                assert!(expression_hash.starts_with("sha256:"));
            }
            other => panic!("expected deny, got {other:?}"),
        }
    }

    #[test]
    fn org_allow_permits_when_no_downstream_deny() {
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
                rules: vec![org_allow_rule()],
                config: ScopeConfig::default(),
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/list".into(),
            tool_name: None,
        };
        let cel = cel_context("tools/list", None);
        let decision = engine.evaluate(&ctx, &cel).unwrap();
        assert!(matches!(decision, HierarchicalDecision::Allow { .. }));
    }

    #[test]
    fn org_deny_sticks_over_lower_allows() {
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
        let cel = cel_context("tools/call", Some("deploy"));
        let decision = engine.evaluate(&ctx, &cel).unwrap();
        assert!(matches!(
            decision,
            HierarchicalDecision::Deny {
                policy_id: Some(id),
                code: rpc_codes::POLICY_DENY,
                ..
            } if id == "org/deny-all"
        ));
    }

    #[test]
    fn default_deny_when_no_bundles() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(Arc::clone(&store), true);
        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: None,
        };
        let cel = cel_context("tools/call", None);
        let decision = engine.evaluate(&ctx, &cel).unwrap();
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
    fn config_scalar_overwrite_and_list_concat() {
        let store = Arc::new(PolicyStore::default());
        let engine = MergeEngine::new(Arc::clone(&store), true);

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
                rules: vec![],
                config: ScopeConfig {
                    spicedb_gate_cel: Some(r#"mcp.method == "tools/call""#.into()),
                    rate_max_requests: Some(100),
                    redaction_paths: vec!["/secrets".into()],
                    audit_tags: vec!["tenant".into()],
                },
            },
        );
        engine.upsert_scope(
            ScopeKey {
                level: PolicyLevel::Server,
                tenant: Some("acme".into()),
                server_group: None,
                server_id: Some("github".into()),
                method: None,
                tool: None,
            },
            ScopeBundle {
                rules: vec![],
                config: ScopeConfig {
                    spicedb_gate_cel: Some(
                        r#"mcp.method == "tools/call" || mcp.method == "resources/read""#.into(),
                    ),
                    rate_max_requests: Some(20),
                    redaction_paths: vec!["/pii".into()],
                    audit_tags: vec!["server".into()],
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
        let effective = engine.effective_policy(&ctx).unwrap();
        assert_eq!(
            effective.config.spicedb_gate_cel.as_deref(),
            Some(r#"mcp.method == "tools/call" || mcp.method == "resources/read""#)
        );
        assert_eq!(effective.config.rate_max_requests, Some(20));
        assert_eq!(
            effective.config.redaction_paths,
            vec!["/secrets".to_string(), "/pii".to_string()]
        );
        assert_eq!(
            effective.config.audit_tags,
            vec!["tenant".to_string(), "server".to_string()]
        );
    }

    #[test]
    fn cache_invalidates_on_scope_change() {
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
                rules: vec![org_allow_rule()],
                config: ScopeConfig::default(),
            },
        );

        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/list".into(),
            tool_name: None,
        };

        let gen_before = engine.generation();
        let first = engine.effective_policy(&ctx).unwrap();
        let second = engine.effective_policy(&ctx).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

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
        assert!(engine.generation() > gen_before);

        let third = engine.effective_policy(&ctx).unwrap();
        assert!(!Arc::ptr_eq(&first, &third));
        assert!(third.deny_rules.iter().any(|r| r.contributor.policy_id == "tenant/acme-block-secrets"));
    }

    #[test]
    fn non_colliding_allows_or_across_layers() {
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
                rules: vec![PolicyRule {
                    policy_id: "org/list".into(),
                    revision: 1,
                    effect: PolicyEffect::Allow,
                    priority: 0,
                    cel: r#"mcp.method == "tools/list""#.into(),
                }],
                config: ScopeConfig::default(),
            },
        );
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
                    policy_id: "tenant/call".into(),
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
            engine.evaluate(&list_ctx, &cel_context("tools/list", None)).unwrap(),
            HierarchicalDecision::Allow { .. }
        ));
        assert!(matches!(
            engine.evaluate(&call_ctx, &cel_context("tools/call", Some("deploy"))).unwrap(),
            HierarchicalDecision::Allow { .. }
        ));
    }

    #[test]
    fn expression_hash_is_stable() {
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
                rules: vec![org_allow_rule()],
                config: ScopeConfig::default(),
            },
        );
        let ctx = MergeRequestContext {
            tenant: "acme".into(),
            server_group: None,
            server_id: "github".into(),
            jsonrpc_method: "tools/list".into(),
            tool_name: None,
        };
        let a = engine.effective_policy(&ctx).unwrap();
        let b = engine.effective_policy(&ctx).unwrap();
        assert_eq!(a.expression_hash, b.expression_hash);
        assert!(a.expression_hash.starts_with("sha256:"));
    }
}
