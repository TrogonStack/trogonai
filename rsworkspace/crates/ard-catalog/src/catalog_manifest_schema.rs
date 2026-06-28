//! Bundled ARD manifest JSON Schema validation.

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

/// Pinned ARD ai-catalog JSON Schema.
///
/// Source: `ards-project/ard-spec` commit
/// `832347bda6af4ce3b61bd250c14a8e899d3ff942`.
pub const AI_CATALOG_JSON_SCHEMA: &str = include_str!("../schemas/ai-catalog.schema.json");

static BUNDLED: OnceLock<AiCatalogJsonSchema> = OnceLock::new();

/// Compiled ARD ai-catalog JSON Schema validator.
pub struct AiCatalogJsonSchema {
    validator: Validator,
}

impl AiCatalogJsonSchema {
    pub fn bundled() -> Self {
        BUNDLED.get_or_init(Self::compile_bundled).clone()
    }

    pub fn validate(&self, doc: &Value) -> Result<(), CatalogManifestValidateError> {
        let errors: Vec<_> = self.validator.iter_errors(doc).collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(CatalogManifestValidateError::from_validation_errors(errors))
        }
    }

    #[allow(clippy::expect_used)]
    fn compile_bundled() -> Self {
        let schema: Value = serde_json::from_str(AI_CATALOG_JSON_SCHEMA).expect("bundled ARD JSON Schema must parse");
        let validator = jsonschema::validator_for(&schema).expect("bundled ARD JSON Schema must compile");
        Self { validator }
    }
}

impl Clone for AiCatalogJsonSchema {
    fn clone(&self) -> Self {
        Self {
            validator: self.validator.clone(),
        }
    }
}

/// A single structured schema violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaViolation {
    instance_path: String,
    schema_path: String,
    message: String,
}

impl SchemaViolation {
    pub fn instance_path(&self) -> &str {
        &self.instance_path
    }

    pub fn schema_path(&self) -> &str {
        &self.schema_path
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Validation failure for an ARD catalog manifest document.
#[derive(Debug, thiserror::Error)]
pub struct CatalogManifestValidateError {
    violations: Vec<SchemaViolation>,
}

impl std::fmt::Display for CatalogManifestValidateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut lines: Vec<String> = self
            .violations
            .iter()
            .map(|v| {
                if v.instance_path.is_empty() {
                    v.message.clone()
                } else {
                    format!("{}: {}", v.instance_path, v.message)
                }
            })
            .collect();
        lines.sort();
        lines.dedup();
        write!(f, "{}", lines.join("; "))
    }
}

impl CatalogManifestValidateError {
    fn from_validation_errors<'a>(errors: impl IntoIterator<Item = jsonschema::ValidationError<'a>>) -> Self {
        let mut violations: Vec<SchemaViolation> = errors
            .into_iter()
            .map(|error| SchemaViolation {
                instance_path: error.instance_path().as_str().to_owned(),
                schema_path: error.schema_path().as_str().to_owned(),
                message: error.to_string(),
            })
            .collect();
        violations.sort_by(|a, b| {
            let a_line = if a.instance_path.is_empty() {
                a.message.clone()
            } else {
                format!("{}: {}", a.instance_path, a.message)
            };
            let b_line = if b.instance_path.is_empty() {
                b.message.clone()
            } else {
                format!("{}: {}", b.instance_path, b.message)
            };
            a_line.cmp(&b_line)
        });
        violations.dedup_by(|a, b| {
            let a_line = if a.instance_path.is_empty() {
                a.message.clone()
            } else {
                format!("{}: {}", a.instance_path, a.message)
            };
            let b_line = if b.instance_path.is_empty() {
                b.message.clone()
            } else {
                format!("{}: {}", b.instance_path, b.message)
            };
            a_line == b_line
        });
        Self { violations }
    }

    pub fn violations(&self) -> &[SchemaViolation] {
        &self.violations
    }
}

/// Validates an ARD catalog manifest value against the bundled schema.
pub fn validate_ai_catalog_value(doc: &Value) -> Result<(), CatalogManifestValidateError> {
    AiCatalogJsonSchema::bundled().validate(doc)
}

#[cfg(test)]
mod tests;
