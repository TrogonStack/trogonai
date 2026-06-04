//! CEL-driven claim-to-attribute mapping.
//!
//! Each [`ClaimMapping`](crate::wif::ClaimMapping) compiles to a CEL program
//! that runs with one bound variable, `assertion`, holding the verified JWT
//! claims as a CEL map. The program's output becomes the value of a derived
//! attribute (e.g. `openai.subject`), which the service-account-mapping
//! resolver matches against.
//!
//! Compilation errors are surfaced once at compile time so a misconfigured
//! provider cannot silently bypass an attribute check.

use std::collections::BTreeMap;

use cel_interpreter::{Context, Program, Value};
use serde_json::Value as JsonValue;

use crate::wif::ClaimMapping;

#[derive(Debug, thiserror::Error)]
pub enum ClaimMappingError {
    #[error("compile `{attribute}`: {message}")]
    Compile { attribute: String, message: String },
    #[error("execute `{attribute}`: {message}")]
    Execute { attribute: String, message: String },
    #[error("attribute `{attribute}` resolved to non-stringable CEL value")]
    NonStringable { attribute: String },
    #[error("bind assertion: {0}")]
    BindAssertion(String),
}

#[derive(Debug)]
pub struct CompiledClaimMapping {
    attribute: String,
    program: Program,
}

impl CompiledClaimMapping {
    pub fn compile(mapping: &ClaimMapping) -> Result<Self, ClaimMappingError> {
        let program = Program::compile(&mapping.expression).map_err(|source| ClaimMappingError::Compile {
            attribute: mapping.attribute.clone(),
            message: source.to_string(),
        })?;
        Ok(Self {
            attribute: mapping.attribute.clone(),
            program,
        })
    }

    #[must_use]
    pub fn attribute(&self) -> &str {
        &self.attribute
    }
}

/// Evaluate the supplied mappings against the verified claims, returning the
/// derived attribute map keyed by the mapping's `attribute` field.
///
/// `assertion` is bound as a CEL map of the verified claims. Mappings see
/// only what's in `claims` — they cannot reach out to the network, the
/// process, or other claim sets.
pub fn evaluate_mappings(
    mappings: &[CompiledClaimMapping],
    claims: &JsonValue,
) -> Result<BTreeMap<String, String>, ClaimMappingError> {
    let mut ctx = Context::default();
    let assertion = cel_interpreter::to_value(claims).map_err(|e| ClaimMappingError::BindAssertion(e.to_string()))?;
    ctx.add_variable_from_value("assertion", assertion);

    let mut out = BTreeMap::new();
    for mapping in mappings {
        let value = mapping.program.execute(&ctx).map_err(|e| ClaimMappingError::Execute {
            attribute: mapping.attribute.clone(),
            message: e.to_string(),
        })?;
        let rendered = stringify_value(&value).ok_or_else(|| ClaimMappingError::NonStringable {
            attribute: mapping.attribute.clone(),
        })?;
        out.insert(mapping.attribute.clone(), rendered);
    }
    Ok(out)
}

fn stringify_value(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.to_string()),
        Value::Int(i) => Some(i.to_string()),
        Value::UInt(u) => Some(u.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => Some(String::new()),
        _ => None,
    }
}

pub fn compile_all(mappings: &[ClaimMapping]) -> Result<Vec<CompiledClaimMapping>, ClaimMappingError> {
    mappings.iter().map(CompiledClaimMapping::compile).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cm(attribute: &str, expression: &str) -> ClaimMapping {
        ClaimMapping {
            attribute: attribute.into(),
            expression: expression.into(),
        }
    }

    #[test]
    fn maps_subject_directly() {
        let compiled = compile_all(&[cm("openai.subject", "assertion.sub")]).unwrap();
        let claims = json!({ "sub": "user-1", "iss": "https://iss" });
        let attrs = evaluate_mappings(&compiled, &claims).unwrap();
        assert_eq!(attrs.get("openai.subject"), Some(&"user-1".to_string()));
    }

    #[test]
    fn maps_composite_repository_ref() {
        let compiled = compile_all(&[cm(
            "openai.repo_ref",
            r#"assertion.repository + "@" + assertion.ref"#,
        )])
        .unwrap();
        let claims = json!({ "repository": "trogonstack/trogonai", "ref": "refs/heads/main" });
        let attrs = evaluate_mappings(&compiled, &claims).unwrap();
        assert_eq!(
            attrs.get("openai.repo_ref"),
            Some(&"trogonstack/trogonai@refs/heads/main".to_string())
        );
    }

    #[test]
    fn integer_claims_stringify() {
        let compiled = compile_all(&[cm("openai.iat", "assertion.iat")]).unwrap();
        let claims = json!({ "iat": 1_700_000_000 });
        let attrs = evaluate_mappings(&compiled, &claims).unwrap();
        assert_eq!(attrs.get("openai.iat"), Some(&"1700000000".to_string()));
    }

    #[test]
    fn missing_claim_fails_loudly() {
        let compiled = compile_all(&[cm("openai.subject", "assertion.sub")]).unwrap();
        let claims = json!({ "iss": "https://iss" });
        let err = evaluate_mappings(&compiled, &claims).unwrap_err();
        assert!(matches!(err, ClaimMappingError::Execute { .. }), "got {err:?}");
    }

    #[test]
    fn list_or_map_result_is_rejected() {
        let compiled = compile_all(&[cm("openai.roles", "assertion.roles")]).unwrap();
        let claims = json!({ "roles": ["admin", "user"] });
        let err = evaluate_mappings(&compiled, &claims).unwrap_err();
        assert!(matches!(err, ClaimMappingError::NonStringable { .. }));
    }
}
