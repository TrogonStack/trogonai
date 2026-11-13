use crate::JsonObject;
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct OutputSchema {
    schema: JsonObject,
    validator: Arc<jsonschema::Validator>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutputSchemaError {
    #[error("invalid JSON schema")]
    Invalid(#[source] Box<jsonschema::ValidationError<'static>>),
    #[error("external JSON schema references require an explicit host-owned resolver")]
    ExternalReference,
}

impl OutputSchema {
    pub fn new(schema: JsonObject) -> Result<Self, OutputSchemaError> {
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
            validator: Arc::new(validator),
        })
    }

    pub fn as_json(&self) -> &JsonObject {
        &self.schema
    }
    pub fn is_valid(&self, instance: &Value) -> bool {
        self.validator.is_valid(instance)
    }

    pub(crate) fn validation_error(&self, instance: &Value) -> Option<String> {
        self.validator.validate(instance).err().map(|error| error.to_string())
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
