use std::fs;
use std::path::Path;

use cel_interpreter::{Program, Value};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use trogon_mcp_gateway::act_chain::ActChainEntry;
use trogon_mcp_gateway::authz::{GatewayIdentity, IdentitySource};
use trogon_mcp_gateway::cel_builtins::HostEvalContext;
use trogon_mcp_gateway::jwt::VerifiedJwtClaims;
use trogon_mcp_gateway::policy::{configure_policy_cel_context, evaluate_cel_with_host, PolicyError};

use crate::output::emit_json;

#[derive(Debug, Deserialize)]
struct DryRunInput {
    identity: IdentityInput,
    #[serde(default)]
    claims: ClaimsInput,
    #[serde(default)]
    act_chain: Vec<ActChainEntry>,
    jsonrpc_method: String,
    #[serde(default)]
    tool_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdentityInput {
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    caller_sub: Option<String>,
    #[serde(default)]
    issuer: Option<String>,
    #[serde(default = "default_identity_source")]
    source: String,
}

fn default_identity_source() -> String {
    "jwt".into()
}

#[derive(Debug, Default, Deserialize)]
struct ClaimsInput {
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    agent_version: Option<String>,
    #[serde(default)]
    wkl: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct PolicyDryRunResult {
    pub decision: &'static str,
    pub value: JsonValue,
    pub expression: String,
}

#[derive(Debug, serde::Serialize)]
pub struct PolicyDryRunError {
    pub error: String,
}

pub fn run(policy_path: &Path, input_path: &Path, pretty: bool) -> Result<(), PolicyOutcome> {
    let expression = fs::read_to_string(policy_path)
        .map_err(|error| PolicyOutcome::Runtime(format!("read policy {}: {error}", policy_path.display())))?;
    let input_raw = fs::read_to_string(input_path)
        .map_err(|error| PolicyOutcome::Runtime(format!("read input {}: {error}", input_path.display())))?;
    let input: DryRunInput = serde_json::from_str(&input_raw)
        .map_err(|error| PolicyOutcome::Runtime(format!("parse input JSON: {error}")))?;

    let program = Program::compile(expression.trim())
        .map_err(|error| PolicyOutcome::Runtime(format!("compile policy: {error}")))?;

    let identity = GatewayIdentity {
        tenant: input.identity.tenant.clone(),
        caller_sub: input.identity.caller_sub.clone(),
        issuer: input.identity.issuer.clone(),
        jti: None,
        source: parse_identity_source(&input.identity.source),
    };
    let claims = VerifiedJwtClaims {
        agent_id: input.claims.agent_id.clone(),
        agent_version: input.claims.agent_version.clone(),
        wkl: input.claims.wkl.clone(),
        wkl_attested_at: None,
        auth_method: None,
        act_chain: if input.act_chain.is_empty() {
            None
        } else {
            Some(input.act_chain.clone())
        },
        purpose: input.claims.purpose.clone(),
        session_id: input.claims.session_id.clone(),
    };

    let host = HostEvalContext::for_tests();
    let value = evaluate_cel_with_host(&program, &host, |ctx| {
        configure_policy_cel_context(ctx, &identity, &claims, &input.act_chain)?;
        let mcp = if let Some(tool) = input.tool_name.as_deref() {
            serde_json::json!({ "method": input.jsonrpc_method, "tool": { "name": tool } })
        } else {
            serde_json::json!({ "method": input.jsonrpc_method })
        };
        let mcp_value = cel_interpreter::to_value(&mcp).map_err(|error| PolicyError(error.to_string()))?;
        ctx.add_variable_from_value("mcp", mcp_value);
        Ok(())
    })
    .map_err(PolicyOutcome::Evaluation)?;

    let decision = match &value {
        Value::Bool(true) => "allow",
        Value::Bool(false) => "deny",
        _ => {
            return Err(PolicyOutcome::Runtime(format!(
                "policy expression must yield bool, got {:?}",
                value.type_of()
            )));
        }
    };

    let payload = PolicyDryRunResult {
        decision,
        value: cel_value_to_json(&value),
        expression: expression.trim().to_string(),
    };
    emit_json(&payload, pretty).map_err(|error| PolicyOutcome::Runtime(error.to_string()))
}

#[derive(Debug)]
pub enum PolicyOutcome {
    Evaluation(PolicyError),
    Runtime(String),
}

impl PolicyOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Evaluation(error) => error.0.clone(),
            Self::Runtime(message) => message.clone(),
        }
    }

    pub fn emit_error_json(&self, pretty: bool) -> Result<(), String> {
        let payload = PolicyDryRunError {
            error: self.message(),
        };
        emit_json(&payload, pretty).map_err(|error| error.to_string())
    }
}

fn parse_identity_source(raw: &str) -> IdentitySource {
    match raw {
        "jwt" => IdentitySource::Jwt,
        "legacy_header" => IdentitySource::LegacyHeader,
        "anonymous" => IdentitySource::Anonymous,
        _ => IdentitySource::Jwt,
    }
}

fn cel_value_to_json(value: &Value) -> JsonValue {
    match value {
        Value::Bool(value) => JsonValue::Bool(*value),
        Value::Int(value) => JsonValue::Number((*value).into()),
        Value::UInt(value) => JsonValue::Number((*value).into()),
        Value::Float(value) => serde_json::Number::from_f64(*value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        Value::String(value) => JsonValue::String(value.to_string()),
        Value::Null => JsonValue::Null,
        other => JsonValue::String(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;
    use tempfile::NamedTempFile;

    fn write_temp(content: &str) -> NamedTempFile {
        let mut file = NamedTempFile::new().expect("temp file");
        write!(file, "{content}").expect("write temp");
        file
    }

    #[test]
    fn dry_run_allow_when_expression_true() {
        let policy = write_temp("true");
        let input = write_temp(
            r#"{
              "identity": { "caller_sub": "alice", "source": "jwt" },
              "jsonrpc_method": "tools/call",
              "tool_name": "deploy"
            }"#,
        );
        run(policy.path(), input.path(), false).expect("allow");
    }

    #[test]
    fn dry_run_deny_when_expression_false() {
        let policy = write_temp(r#"jwt.sub == "bob""#);
        let input = write_temp(
            r#"{
              "identity": { "caller_sub": "alice", "source": "jwt" },
              "jsonrpc_method": "tools/list"
            }"#,
        );
        run(policy.path(), input.path(), false).expect("deny outcome still success exit");
    }
}
