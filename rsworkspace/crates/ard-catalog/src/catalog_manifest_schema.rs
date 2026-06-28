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

/// Validation failure for an ARD catalog manifest document.
#[derive(Debug, thiserror::Error)]
#[error("{}", .details.join("; "))]
pub struct CatalogManifestValidateError {
    details: Vec<String>,
}

impl CatalogManifestValidateError {
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

    pub fn details(&self) -> &[String] {
        &self.details
    }
}

/// Validates an ARD catalog manifest value against the bundled schema.
pub fn validate_ai_catalog_value(doc: &Value) -> Result<(), CatalogManifestValidateError> {
    AiCatalogJsonSchema::bundled().validate(doc)
}

#[cfg(test)]
mod tests;
