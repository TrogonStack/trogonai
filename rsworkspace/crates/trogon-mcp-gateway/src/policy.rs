//! CEL policy: selects when the SpiceDB-backed authorization hook runs (`tools/call`, `resources/read`).

use cel_interpreter::{Context, Program, Value};

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

/// Build the `jwt` object for CEL evaluation with nullable agent-identity claims.
#[must_use]
pub fn jwt_cel_object(identity: &GatewayIdentity, claims: &VerifiedJwtClaims) -> serde_json::Value {
    serde_json::json!({
        "sub": identity.caller_sub,
        "tenant": identity.tenant,
        "iss": identity.issuer,
        "agent_id": claims.agent_id,
        "agent_version": claims.agent_version,
        "wkl": claims.wkl,
        "purpose": claims.purpose,
        "session_id": claims.session_id,
    })
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
        add_jwt_to_cel_context(
            &mut ctx,
            jwt_cel_object(
                &GatewayIdentity {
                    tenant: Some("acme".into()),
                    caller_sub: Some("alice".into()),
                    issuer: None,
                    jti: None,
                    source: IdentitySource::Jwt,
                },
                &VerifiedJwtClaims {
                    agent_id: Some("oncall-agent".into()),
                    ..VerifiedJwtClaims::default()
                },
            ),
        )
        .unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn jwt_agent_id_is_visible_to_cel() {
        let program = Program::compile(r#"jwt.agent_id == "oncall-agent""#).unwrap();
        let mut ctx = Context::default();
        add_jwt_to_cel_context(
            &mut ctx,
            jwt_cel_object(
                &GatewayIdentity {
                    tenant: None,
                    caller_sub: Some("alice".into()),
                    issuer: None,
                    jti: None,
                    source: IdentitySource::Jwt,
                },
                &VerifiedJwtClaims {
                    agent_id: Some("oncall-agent".into()),
                    ..VerifiedJwtClaims::default()
                },
            ),
        )
        .unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }

    #[test]
    fn jwt_agent_id_null_when_absent() {
        let program = Program::compile(r#"jwt.agent_id == null"#).unwrap();
        let mut ctx = Context::default();
        add_jwt_to_cel_context(
            &mut ctx,
            jwt_cel_object(
                &GatewayIdentity {
                    tenant: None,
                    caller_sub: Some("alice".into()),
                    issuer: None,
                    jti: None,
                    source: IdentitySource::Jwt,
                },
                &VerifiedJwtClaims::default(),
            ),
        )
        .unwrap();
        assert!(matches!(program.execute(&ctx), Ok(Value::Bool(true))));
    }
}
