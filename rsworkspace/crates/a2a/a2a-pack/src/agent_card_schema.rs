//! AgentCard JSON Schema for registration-time validation.
//!
//! Guards writes to the agent catalog KV bucket before cards are published.

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

    #[allow(clippy::expect_used)]
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
#[derive(Debug, thiserror::Error)]
#[error("{}", .details.join("; "))]
pub struct AgentCardValidateError {
    details: Vec<String>,
}

impl AgentCardValidateError {
    fn from_validation_errors<'a>(errors: impl IntoIterator<Item = jsonschema::ValidationError<'a>>) -> Self {
        let mut details: Vec<String> = errors
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
        details.sort();
        details.dedup();
        Self { details }
    }

    /// Individual schema validation failures, one entry per offending location.
    pub fn details(&self) -> &[String] {
        &self.details
    }
}

/// Validates an AgentCard value against the bundled schema.
pub fn validate_agent_card_value(doc: &Value) -> Result<(), AgentCardValidateError> {
    AgentCardJsonSchema::bundled().validate(doc)
}

#[cfg(test)]
mod tests;
