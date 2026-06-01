//! CEL policy: selects when the SpiceDB-backed authorization hook runs (`tools/call`, `resources/read`).

pub mod hierarchical;
pub mod list_filter;

use cel_interpreter::extractors::This;
use cel_interpreter::objects::Key;
use cel_interpreter::{Context, Program, Value, functions};
use serde_json::Value as JsonValue;

use crate::act_chain::ActChainEntry;
use crate::approvals::{build_approval_required, build_approval_required_step_up};
use crate::authz::GatewayIdentity;
use crate::cel_builtins::{classify_list_filter_host_error, register_all, with_host_eval, HostEvalContext, HostFailure};
use crate::jwt::VerifiedJwtClaims;
use crate::rpc_codes;
use crate::throttle::{ContextThrottler, ThrottleConfig, ThrottleKey};

const SPICEDB_GATE_EXPR: &str = r#"mcp.method == "tools/call" || mcp.method == "resources/read""#;

const DEFAULT_APPROVAL_TTL_SECS: u64 = 300;
const DEFAULT_APPROVAL_BASE_URL: &str = "https://console.trogon.local";

#[derive(Clone, Debug)]
pub struct ThrottleMeshConfig {
    pub enabled: bool,
    pub window_secs: u64,
    pub max_requests: u32,
}

impl Default for ThrottleMeshConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_secs: 300,
            max_requests: 100,
        }
    }
}

impl From<&ThrottleMeshConfig> for ThrottleConfig {
    fn from(value: &ThrottleMeshConfig) -> Self {
        Self {
            enabled: value.enabled,
            window_secs: value.window_secs,
            max_requests: value.max_requests,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RiskThresholds {
    pub approval_score: u32,
    pub deny_score: u32,
    pub step_up_purposes: Vec<String>,
    pub approval_denials_60s: u32,
}

impl Default for RiskThresholds {
    fn default() -> Self {
        Self {
            approval_score: 80,
            deny_score: 120,
            step_up_purposes: vec!["privileged".into(), "admin".into()],
            approval_denials_60s: 3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MeshGatewayConfig {
    pub throttle: ThrottleMeshConfig,
    pub risk: RiskThresholds,
    pub approval_ttl_secs: u64,
    pub approval_base_url: String,
}

impl Default for MeshGatewayConfig {
    fn default() -> Self {
        Self {
            throttle: ThrottleMeshConfig::default(),
            risk: RiskThresholds::default(),
            approval_ttl_secs: DEFAULT_APPROVAL_TTL_SECS,
            approval_base_url: DEFAULT_APPROVAL_BASE_URL.into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct CallContext {
    pub tenant: String,
    pub agent_id: String,
    pub purpose: String,
    pub target_aud: String,
    pub scope_fingerprint: String,
    pub jsonrpc_method: String,
    pub tool_name: Option<String>,
    pub recent_denials_60s: u32,
    pub args: JsonValue,
    pub request_id: String,
}

impl CallContext {
    pub fn throttle_key(&self) -> ThrottleKey {
        ThrottleKey {
            tenant: self.tenant.clone(),
            agent_id: self.agent_id.clone(),
            purpose: self.purpose.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RiskDecision {
    Allow,
    Deny { reason: String },
    StepUp { scope: String },
    RequireApproval { reason: String, ttl_s: u64 },
    Throttle { retry_after_s: u64 },
}

#[derive(Clone, Debug)]
pub struct PolicyOutcome {
    pub requires_spicedb: bool,
    pub risk: RiskDecision,
}

#[derive(Clone, Debug)]
pub struct RiskGateResponse {
    pub requires_spicedb: bool,
    pub risk: RiskDecision,
    pub approval_data: Option<JsonValue>,
}

#[derive(Debug)]
pub struct PolicyError(pub String);

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PolicyError {}

#[derive(Debug)]
pub struct SpicedbGatePolicy {
    program: Program,
}

impl SpicedbGatePolicy {
    pub fn phase1_hardcoded() -> Result<Self, PolicyError> {
        Self::from_cel(SPICEDB_GATE_EXPR)
    }

    pub fn from_cel(expr: &str) -> Result<Self, PolicyError> {
        Program::compile(expr)
            .map(|program| Self { program })
            .map_err(|e| PolicyError(e.to_string()))
    }

    pub fn from_effective_config(config: &hierarchical::MergedConfig) -> Result<Self, PolicyError> {
        match config.spicedb_gate_cel.as_deref() {
            Some(expr) => Self::from_cel(expr),
            None => Self::phase1_hardcoded(),
        }
    }

    pub fn requires_spicedb_for_method(&self, jsonrpc_method: &str) -> Result<bool, PolicyError> {
        let mut ctx = Context::default();
        let mcp = serde_json::json!({ "method": jsonrpc_method });
        let value = cel_interpreter::to_value(&mcp).map_err(|e| PolicyError(e.to_string()))?;
        ctx.add_variable_from_value("mcp", value);
        match self.program.execute(&ctx) {
            Ok(Value::Bool(b)) => Ok(b),
            Ok(other) => Err(PolicyError(format!(
                "policy expression must yield bool, got {:?}",
                other.type_of()
            ))),
            Err(e) => Err(PolicyError(e.to_string())),
        }
    }
}

fn act_chain_entry_json(entry: &ActChainEntry) -> serde_json::Value {
    serde_json::json!({
        "sub": entry.sub,
        "agent_id": entry.agent_id,
        "wkl": entry.wkl,
        "iat": entry.iat,
    })
}

/// Serialize parsed act-chain hops for CEL (`list<map>`).
#[must_use]
pub fn act_chain_cel_value(entries: &[ActChainEntry]) -> serde_json::Value {
    serde_json::Value::Array(entries.iter().map(act_chain_entry_json).collect())
}

/// Build the `jwt` object for CEL evaluation with nullable agent-identity claims.
#[must_use]
pub fn jwt_cel_object(
    identity: &GatewayIdentity,
    claims: &VerifiedJwtClaims,
    act_chain: &[ActChainEntry],
) -> serde_json::Value {
    serde_json::json!({
        "sub": identity.caller_sub,
        "tenant": identity.tenant,
        "iss": identity.issuer,
        "agent_id": claims.agent_id,
        "agent_version": claims.agent_version,
        "wkl": claims.wkl,
        "purpose": claims.purpose,
        "session_id": claims.session_id,
        "act_chain": act_chain_cel_value(act_chain),
    })
}

fn register_act_chain_cel_helpers(ctx: &mut Context) {
    ctx.add_function("contains", act_chain_aware_contains);
    ctx.add_function("depth", act_chain_depth);
    ctx.add_function("originator", act_chain_originator);
}

fn act_chain_aware_contains(This(this): This<Value>, arg: Value) -> Result<Value, cel_interpreter::ExecutionError> {
    if let Value::List(entries) = &this {
        let needle = match &arg {
            Value::String(s) => s.as_str(),
            _ => return functions::contains(This(this.clone()), arg),
        };
        if entries.iter().all(|entry| matches!(entry, Value::Map(_))) {
            let found = entries
                .iter()
                .any(|entry| act_chain_entry_agent_id(entry) == Some(needle));
            return Ok(found.into());
        }
    }
    functions::contains(This(this), arg)
}

fn act_chain_depth(
    ftx: &cel_interpreter::FunctionContext,
    This(this): This<Value>,
) -> Result<i64, cel_interpreter::ExecutionError> {
    match this {
        Value::List(list) => Ok(list.len() as i64),
        other => Err(ftx.error(format!("cannot determine depth of {other:?}"))),
    }
}

fn act_chain_originator(
    ftx: &cel_interpreter::FunctionContext,
    This(this): This<Value>,
) -> Result<Value, cel_interpreter::ExecutionError> {
    match this {
        Value::List(list) if list.is_empty() => Ok(Value::Null),
        Value::List(list) => Ok(list[0].clone()),
        other => Err(ftx.error(format!("cannot determine originator of {other:?}"))),
    }
}

fn act_chain_entry_agent_id(entry: &Value) -> Option<&str> {
    let Value::Map(map) = entry else {
        return None;
    };
    match map.get(&Key::from("agent_id"))? {
        Value::String(agent_id) if !agent_id.is_empty() => Some(agent_id.as_str()),
        _ => None,
    }
}

/// Bind `jwt`, `chain`, act-chain helpers, and host builtins for policy evaluation.
pub fn configure_policy_cel_context(
    ctx: &mut Context,
    identity: &GatewayIdentity,
    claims: &VerifiedJwtClaims,
    act_chain: &[ActChainEntry],
) -> Result<(), PolicyError> {
    register_act_chain_cel_helpers(ctx);

    let jwt = jwt_cel_object(identity, claims, act_chain);
    let value = cel_interpreter::to_value(&jwt).map_err(|e| PolicyError(e.to_string()))?;
    ctx.add_variable_from_value("jwt", value);

    let chain = act_chain_cel_value(act_chain);
    let chain_value = cel_interpreter::to_value(&chain).map_err(|e| PolicyError(e.to_string()))?;
    ctx.add_variable_from_value("chain", chain_value);

    register_all(ctx).map_err(|e| PolicyError(e.to_string()))?;
    Ok(())
}

/// Fresh CEL context with standard functions, policy variables, act-chain helpers, and `mcp` bindings.
pub fn new_policy_cel_context_for_request(
    identity: &GatewayIdentity,
    claims: &VerifiedJwtClaims,
    act_chain: &[ActChainEntry],
    jsonrpc_method: &str,
    tool_name: Option<&str>,
) -> Result<Context<'static>, PolicyError> {
    let mut ctx = new_policy_cel_context(identity, claims, act_chain)?;
    let mcp = if let Some(tool) = tool_name {
        serde_json::json!({ "method": jsonrpc_method, "tool": { "name": tool } })
    } else {
        serde_json::json!({ "method": jsonrpc_method })
    };
    let value = cel_interpreter::to_value(&mcp).map_err(|e| PolicyError(e.to_string()))?;
    ctx.add_variable_from_value("mcp", value);
    Ok(ctx)
}

/// Execute a compiled CEL program with host builtins and request-scoped host state.
pub fn evaluate_cel_with_host(
    program: &Program,
    host: &HostEvalContext,
    configure: impl FnOnce(&mut Context) -> Result<(), PolicyError>,
) -> Result<Value, PolicyError> {
    let mut ctx = Context::default();
    configure(&mut ctx)?;
    with_host_eval(host, || {
        program
            .execute(&ctx)
            .map_err(|e| PolicyError(e.to_string()))
    })
}

/// Outcome of host-aware CEL evaluation for per-tool list filtering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CelHostEvalOutcome {
    Bool(bool),
    HostFailure(HostFailure, String),
    Runtime(String),
}

/// Evaluate CEL with host builtins, preserving host failure classification for list shaping.
pub fn evaluate_cel_with_host_classified(
    program: &Program,
    host: &HostEvalContext,
    configure: impl FnOnce(&mut Context) -> Result<(), PolicyError>,
) -> CelHostEvalOutcome {
    let mut ctx = Context::default();
    if let Err(err) = configure(&mut ctx) {
        return CelHostEvalOutcome::Runtime(err.0);
    }

    match with_host_eval(host, || program.execute(&ctx)) {
        Ok(Value::Bool(value)) => CelHostEvalOutcome::Bool(value),
        Ok(other) => CelHostEvalOutcome::Runtime(format!(
            "policy expression must yield bool, got {:?}",
            other.type_of()
        )),
        Err(err) => {
            let detail = err.to_string();
            if let Some(failure) = classify_list_filter_host_error(&detail) {
                CelHostEvalOutcome::HostFailure(failure, detail)
            } else {
                CelHostEvalOutcome::Runtime(detail)
            }
        }
    }
}

/// Fresh CEL context with standard functions, policy variables, and act-chain helpers.
pub fn new_policy_cel_context(
    identity: &GatewayIdentity,
    claims: &VerifiedJwtClaims,
    act_chain: &[ActChainEntry],
) -> Result<Context<'static>, PolicyError> {
    let mut ctx = Context::default();
    configure_policy_cel_context(&mut ctx, identity, claims, act_chain)?;
    Ok(ctx)
}

pub fn add_jwt_to_cel_context(ctx: &mut Context, jwt: serde_json::Value) -> Result<(), PolicyError> {
    let value = cel_interpreter::to_value(&jwt).map_err(|e| PolicyError(e.to_string()))?;
    ctx.add_variable_from_value("jwt", value);
    Ok(())
}

fn risk_score(ctx: &CallContext, thresholds: &RiskThresholds) -> u32 {
    let mut score = 0u32;
    score = score.saturating_add(ctx.recent_denials_60s.saturating_mul(20));
    if ctx.recent_denials_60s >= thresholds.approval_denials_60s {
        score = score.saturating_add(40);
    }
    if thresholds
        .step_up_purposes
        .iter()
        .any(|purpose| purpose.eq_ignore_ascii_case(ctx.purpose.as_str()))
    {
        score = score.saturating_add(50);
    }
    if ctx.jsonrpc_method == "tools/call" {
        score = score.saturating_add(10);
    }
    if ctx.scope_fingerprint.is_empty() {
        score = score.saturating_add(15);
    }
    if ctx.target_aud.contains(":backend:") {
        score = score.saturating_add(5);
    }
    score
}

#[must_use]
pub fn evaluate_risk(ctx: &CallContext, config: &MeshGatewayConfig) -> RiskDecision {
    let score = risk_score(ctx, &config.risk);
    if score >= config.risk.deny_score {
        return RiskDecision::Deny {
            reason: format!("risk_score={score}"),
        };
    }
    if thresholds_require_step_up(ctx, &config.risk) {
        return RiskDecision::StepUp {
            scope: format!("purpose:{}", ctx.purpose),
        };
    }
    if score >= config.risk.approval_score {
        return RiskDecision::RequireApproval {
            reason: format!("risk_score={score}"),
            ttl_s: config.approval_ttl_secs,
        };
    }
    RiskDecision::Allow
}

fn thresholds_require_step_up(ctx: &CallContext, thresholds: &RiskThresholds) -> bool {
    thresholds
        .step_up_purposes
        .iter()
        .any(|purpose| purpose.eq_ignore_ascii_case(ctx.purpose.as_str()))
        && ctx.jsonrpc_method == "tools/call"
}

pub fn run_with_risk(
    policy: &SpicedbGatePolicy,
    mesh_config: &MeshGatewayConfig,
    ctx: &CallContext,
    throttler: &ContextThrottler,
) -> Result<PolicyOutcome, PolicyError> {
    let requires_spicedb = policy.requires_spicedb_for_method(&ctx.jsonrpc_method)?;
    if let Some(retry_after_s) = throttler.check_and_record(&ctx.throttle_key()) {
        return Ok(PolicyOutcome {
            requires_spicedb,
            risk: RiskDecision::Throttle { retry_after_s },
        });
    }
    Ok(PolicyOutcome {
        requires_spicedb,
        risk: evaluate_risk(ctx, mesh_config),
    })
}

#[must_use]
pub fn risk_gate_response(
    prefix: &str,
    mesh_config: &MeshGatewayConfig,
    ctx: &CallContext,
    outcome: &PolicyOutcome,
) -> RiskGateResponse {
    use crate::approvals::RequestId;

    let approval_data = match &outcome.risk {
        RiskDecision::StepUp { scope } => RequestId::new(&ctx.request_id).ok().map(|request_id| {
            build_approval_required_step_up(
                prefix,
                &request_id,
                scope,
                mesh_config.approval_ttl_secs,
                mesh_config.approval_base_url.as_str(),
            )
        }),
        RiskDecision::RequireApproval { reason, ttl_s } => {
            RequestId::new(&ctx.request_id).ok().map(|request_id| {
                build_approval_required(
                    prefix,
                    &request_id,
                    reason,
                    *ttl_s,
                    mesh_config.approval_base_url.as_str(),
                )
            })
        }
        _ => None,
    };
    RiskGateResponse {
        requires_spicedb: outcome.requires_spicedb,
        risk: outcome.risk.clone(),
        approval_data,
    }
}

#[must_use]
pub fn risk_decision_blocks_request(risk: &RiskDecision) -> bool {
    !matches!(risk, RiskDecision::Allow)
}

#[must_use]
pub fn risk_decision_jsonrpc_code(risk: &RiskDecision) -> i32 {
    match risk {
        RiskDecision::Allow => 0,
        RiskDecision::Deny { .. } => rpc_codes::POLICY_DENY,
        RiskDecision::StepUp { .. } | RiskDecision::RequireApproval { .. } => rpc_codes::APPROVAL_REQUIRED,
        RiskDecision::Throttle { .. } => rpc_codes::RATE_LIMITED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::IdentitySource;
    use crate::jwt::VerifiedJwtClaims;

    fn sample_identity() -> GatewayIdentity {
        GatewayIdentity {
            tenant: None,
            caller_sub: Some("alice".into()),
            issuer: None,
            jti: None,
            source: IdentitySource::Jwt,
        }
    }

    fn two_hop_chain() -> Vec<ActChainEntry> {
        vec![
            ActChainEntry {
                sub: "user:alice".into(),
                agent_id: None,
                wkl: None,
                iat: 100,
            },
            ActChainEntry {
                sub: "agent:acme/oncall".into(),
                agent_id: Some("agent:acme/oncall".into()),
                wkl: None,
                iat: 200,
            },
        ]
    }

    #[test]
    fn gate_is_true_for_tools_call() {
        let p = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(p.requires_spicedb_for_method("tools/call").unwrap());
    }

    #[test]
    fn gate_is_true_for_resources_read() {
        let p = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(p.requires_spicedb_for_method("resources/read").unwrap());
    }

    #[test]
    fn gate_is_false_for_list_tools() {
        let p = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        assert!(!p.requires_spicedb_for_method("tools/list").unwrap());
    }

    #[test]
    fn phase1_gate_ignores_jwt_variables() {
        let program = Program::compile(SPICEDB_GATE_EXPR).unwrap();
        let mut ctx = Context::default();
        let mcp = cel_interpreter::to_value(serde_json::json!({ "method": "tools/call" })).unwrap();
        ctx.add_variable_from_value("mcp", mcp);
        configure_policy_cel_context(
            &mut ctx,
            &sample_identity(),
            &VerifiedJwtClaims {
                agent_id: Some("oncall-agent".into()),
                ..VerifiedJwtClaims::default()
            },
            &[],
        )
        .unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn jwt_agent_id_is_visible_to_cel() {
        let program = Program::compile(r#"jwt.agent_id == "oncall-agent""#).unwrap();
        let ctx = new_policy_cel_context(
            &sample_identity(),
            &VerifiedJwtClaims {
                agent_id: Some("oncall-agent".into()),
                ..VerifiedJwtClaims::default()
            },
            &[],
        )
        .unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn jwt_agent_id_null_when_absent() {
        let program = Program::compile(r#"jwt.agent_id == null"#).unwrap();
        let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &[]).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn jwt_act_chain_size_rule() {
        let program = Program::compile("jwt.act_chain.size() >= 2").unwrap();
        let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &two_hop_chain()).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn chain_contains_matches_agent_id() {
        let program = Program::compile(r#"chain.contains("agent:acme/oncall")"#).unwrap();
        let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &two_hop_chain()).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));

        let program = Program::compile(r#"chain.contains("agent:missing")"#).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(false))));
    }

    #[test]
    fn chain_originator_sub() {
        let program = Program::compile(r#"chain.originator().sub == "user:alice""#).unwrap();
        let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &two_hop_chain()).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn chain_depth_counts_entries() {
        let program = Program::compile("chain.depth() == 2").unwrap();
        let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &two_hop_chain()).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn policies_without_act_chain_still_run() {
        let program = Program::compile(r#"jwt.sub == "alice" && jwt.agent_id == null"#).unwrap();
        let ctx = new_policy_cel_context(&sample_identity(), &VerifiedJwtClaims::default(), &[]).unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    fn sample_call_context(recent_denials: u32, purpose: &str) -> CallContext {
        CallContext {
            tenant: "acme".into(),
            agent_id: "agent/oncall".into(),
            purpose: purpose.into(),
            target_aud: "urn:trogon:mcp:backend:acme:github".into(),
            scope_fingerprint: "tool:deploy".into(),
            jsonrpc_method: "tools/call".into(),
            tool_name: Some("deploy".into()),
            recent_denials_60s: recent_denials,
            args: serde_json::json!({"name": "deploy"}),
            request_id: "req-policy-test".into(),
        }
    }

    #[test]
    fn low_risk_allows() {
        let ctx = sample_call_context(0, "routine");
        assert_eq!(evaluate_risk(&ctx, &MeshGatewayConfig::default()), RiskDecision::Allow);
    }

    #[test]
    fn privileged_purpose_requires_step_up() {
        let ctx = sample_call_context(0, "admin");
        assert_eq!(
            evaluate_risk(&ctx, &MeshGatewayConfig::default()),
            RiskDecision::StepUp {
                scope: "purpose:admin".into()
            }
        );
    }

    #[test]
    fn repeated_denials_require_approval() {
        let ctx = sample_call_context(3, "routine");
        match evaluate_risk(&ctx, &MeshGatewayConfig::default()) {
            RiskDecision::RequireApproval { reason, .. } => assert!(reason.contains("risk_score")),
            other => panic!("expected RequireApproval, got {other:?}"),
        }
    }

    #[test]
    fn extreme_denials_deny() {
        let mut config = MeshGatewayConfig::default();
        config.risk.deny_score = 60;
        let ctx = sample_call_context(10, "routine");
        assert!(matches!(evaluate_risk(&ctx, &config), RiskDecision::Deny { .. }));
    }

    #[test]
    fn run_with_risk_applies_throttle() {
        let policy = SpicedbGatePolicy::phase1_hardcoded().unwrap();
        let mesh = MeshGatewayConfig {
            throttle: ThrottleMeshConfig {
                enabled: true,
                window_secs: 60,
                max_requests: 1,
            },
            ..MeshGatewayConfig::default()
        };
        let throttler = ContextThrottler::new((&mesh.throttle).into());
        let ctx = sample_call_context(0, "routine");
        let first = run_with_risk(&policy, &mesh, &ctx, &throttler).unwrap();
        assert_eq!(first.risk, RiskDecision::Allow);
        let second = run_with_risk(&policy, &mesh, &ctx, &throttler).unwrap();
        assert!(matches!(second.risk, RiskDecision::Throttle { .. }));
    }

    #[test]
    fn risk_gate_response_builds_approval_envelope() {
        let ctx = sample_call_context(0, "admin");
        let mesh = MeshGatewayConfig::default();
        let outcome = PolicyOutcome {
            requires_spicedb: true,
            risk: evaluate_risk(&ctx, &mesh),
        };
        let response = risk_gate_response("mcp", &mesh, &ctx, &outcome);
        let data = response.approval_data.expect("approval data");
        assert_eq!(data["approval_subject"], "mcp.approvals.step-up.req-policy-test");
    }
}
