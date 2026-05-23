//! AgentCard JSON Schema for registration-time validation.
//!
//! Guards writes to the agent catalog KV bucket before cards are published.

use std::fmt;
use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

/// Bundled draft-07 JSON Schema for minimal AgentCard registration checks.
pub const AGENT_CARD_JSON_SCHEMA: &str = include_str!("../schemas/agent-card.min.json");

static BUNDLED: OnceLock<AgentCardJsonSchema> = OnceLock::new();

/// Compiled AgentCard JSON Schema validator.
pub struct AgentCardJsonSchema {
    validator: Validator,
}

impl AgentCardJsonSchema {
    /// Compiles the bundled schema once per process.
    pub fn bundled() -> Self {
        BUNDLED.get_or_init(Self::compile_bundled).clone()
    }

    /// Validates a parsed AgentCard document.
    pub fn validate(&self, doc: &Value) -> Result<(), AgentCardValidateError> {
        let errors: Vec<_> = self.validator.iter_errors(doc).collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(AgentCardValidateError::from_validation_errors(errors))
        }
    }

    fn compile_bundled() -> Self {
        let schema: Value =
            serde_json::from_str(AGENT_CARD_JSON_SCHEMA).expect("bundled AgentCard JSON Schema must parse");
        let validator = jsonschema::validator_for(&schema).expect("bundled AgentCard JSON Schema must compile");
        Self { validator }
    }
}

impl Clone for AgentCardJsonSchema {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
        }
    }
}

/// Validation failure for an AgentCard document.
#[derive(Debug)]
pub struct AgentCardValidateError {
    message: String,
}

impl AgentCardValidateError {
    fn from_validation_errors<'a>(errors: impl IntoIterator<Item = jsonschema::ValidationError<'a>>) -> Self {
        let mut lines: Vec<String> = errors
            .into_iter()
            .map(|error| {
                let path = error.instance_path().as_str();
                if path.is_empty() {
                    error.to_string()
                } else {
                    format!("{path}: {error}")
                }
            })
            .collect();
        lines.sort();
        lines.dedup();
        let message = if lines.is_empty() {
            "AgentCard failed JSON Schema validation".to_string()
        } else {
            lines.join("; ")
        };
        Self { message }
    }
}

impl fmt::Display for AgentCardValidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for AgentCardValidateError {}

/// Validates an AgentCard value against the bundled schema.
pub fn validate_agent_card_value(doc: &Value) -> Result<(), AgentCardValidateError> {
    AgentCardJsonSchema::bundled().validate(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal_valid() -> Value {
        json!({
            "name": "my-agent",
            "supportedInterfaces": [{
                "url": "https://example.com/a2a",
                "protocolBinding": "JSONRPC",
                "protocolVersion": "0.2.0"
            }]
        })
    }

    #[test]
    fn accepts_minimal_valid_json() {
        validate_agent_card_value(&minimal_valid()).expect("minimal card should validate");
    }

    #[test]
    fn rejects_empty_object() {
        let err = validate_agent_card_value(&json!({})).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn rejects_empty_name() {
        let mut card = minimal_valid();
        card["name"] = json!("");
        let err = validate_agent_card_value(&card).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    #[test]
    fn rejects_missing_primary_interface_protocol_version() {
        let card = json!({
            "name": "my-agent",
            "supportedInterfaces": [{
                "url": "https://example.com/a2a",
                "protocolBinding": "JSONRPC"
            }]
        });
        let err = validate_agent_card_value(&card).unwrap_err();
        assert!(err.to_string().contains("protocolVersion"));
    }

    #[test]
    fn rejects_missing_supported_interfaces_array() {
        let card = json!({
            "name": "my-agent",
        });
        let err = validate_agent_card_value(&card).unwrap_err();
        assert!(err.to_string().contains("supportedInterfaces"));
    }

    #[test]
    fn rejects_non_http_interface_url() {
        let mut card = minimal_valid();
        card["supportedInterfaces"][0]["url"] = json!("nats://example/a2a");
        let err = validate_agent_card_value(&card).unwrap_err();
        assert!(err.to_string().contains("url"));
    }
}
