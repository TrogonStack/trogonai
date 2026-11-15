use crate::JsonObject;
use serde_json::Value;
use std::sync::Arc;

/// A JSON schema the SDK sends to the CLI verbatim and can validate instances against.
///
/// Both upstream SDKs pass schemas through without checking them, so `OutputSchema::new` keeps any
/// object. A schema that cannot be compiled, for example one whose `$ref` points outside the
/// document, validates nothing and reports its compile error where a tool call is checked.
/// `OutputSchema::validated` is an opt-in check that refuses such schemas when they are built.
#[derive(Clone)]
pub struct OutputSchema {
    schema: JsonObject,
    validator: Arc<Result<jsonschema::Validator, String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputSchemaError {
    #[error("invalid JSON schema")]
    Invalid(#[source] Box<jsonschema::ValidationError<'static>>),
    #[error("external JSON schema references require an explicit host-owned resolver")]
    ExternalReference,
}

impl OutputSchema {
    pub fn new(schema: JsonObject) -> Self {
        let validator = jsonschema::validator_for(&Value::Object(schema.clone())).map_err(|error| error.to_string());
        Self {
            schema,
            validator: Arc::new(validator),
        }
    }

    /// Opt-in check that neither upstream SDK performs: refuses a schema that references anything
    /// outside the document or that does not compile.
    pub fn validated(schema: JsonObject) -> Result<Self, OutputSchemaError> {
        fn local_references(value: &Value) -> bool {
            match value {
                Value::Object(object) => object.iter().all(|(key, value)| {
                    if matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                        value.as_str().is_some_and(|reference| reference.starts_with('#'))
                    } else {
                        local_references(value)
                    }
                }),
                Value::Array(values) => values.iter().all(local_references),
                _ => true,
            }
        }
        let value = Value::Object(schema.clone());
        if !local_references(&value) {
            return Err(OutputSchemaError::ExternalReference);
        }
        let validator =
            jsonschema::validator_for(&value).map_err(|error| OutputSchemaError::Invalid(Box::new(error)))?;
        Ok(Self {
            schema,
            validator: Arc::new(Ok(validator)),
        })
    }

    pub fn as_json(&self) -> &JsonObject {
        &self.schema
    }

    /// Whether `instance` matches the schema. A schema that did not compile matches nothing.
    pub fn is_valid(&self, instance: &Value) -> bool {
        self.validator
            .as_ref()
            .as_ref()
            .is_ok_and(|validator| validator.is_valid(instance))
    }

    /// The compile error of a schema that `OutputSchema::new` accepted but could not compile.
    pub fn compile_error(&self) -> Option<&str> {
        self.validator.as_ref().as_ref().err().map(String::as_str)
    }

    pub(crate) fn validation_error(&self, instance: &Value) -> Option<String> {
        match self.validator.as_ref() {
            Ok(validator) => validator.validate(instance).err().map(|error| error.to_string()),
            Err(error) => Some(error.clone()),
        }
    }
}

impl From<JsonObject> for OutputSchema {
    fn from(schema: JsonObject) -> Self {
        Self::new(schema)
    }
}

impl std::fmt::Debug for OutputSchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OutputSchema { .. }")
    }
}

#[cfg(test)]
#[path = "output_schema_tests.rs"]
mod tests;
