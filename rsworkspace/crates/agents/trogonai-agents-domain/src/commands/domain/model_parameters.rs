use std::collections::BTreeMap;

use super::nonblank::validate_nonblank;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelParameters(BTreeMap<String, String>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("model parameter key '{key}' must be nonblank and trimmed")]
pub struct ModelParametersError {
    key: String,
}

impl ModelParameters {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, ModelParametersError> {
        for key in values.keys() {
            if validate_nonblank(key).is_err() {
                return Err(ModelParametersError { key: key.clone() });
            }
        }
        Ok(Self(values))
    }

    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }
}

#[cfg(test)]
mod tests;
