//! CEL policy: selects when the SpiceDB-backed authorization hook runs (`tools/call`, `resources/read`).

use cel_interpreter::extractors::This;
use cel_interpreter::objects::Key;
use cel_interpreter::{Context, Program, Value, functions};

use crate::act_chain::ActChainEntry;
use crate::authz::GatewayIdentity;
use crate::jwt::VerifiedJwtClaims;

const SPICEDB_GATE_EXPR: &str = r#"mcp.method == "tools/call" || mcp.method == "resources/read""#;

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
        Program::compile(SPICEDB_GATE_EXPR)
            .map(|program| Self { program })
            .map_err(|e| PolicyError(e.to_string()))
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

/// Bind `jwt`, `chain`, and act-chain helper functions for policy evaluation.
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
    Ok(())
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
}
