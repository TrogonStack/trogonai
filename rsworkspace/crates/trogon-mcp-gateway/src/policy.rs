//! CEL policy: selects when the SpiceDB-backed authorization hook runs (`tools/call`, `resources/read`).

use cel_interpreter::{Context, Program, Value};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
