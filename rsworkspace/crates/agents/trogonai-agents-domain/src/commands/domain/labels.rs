use std::collections::BTreeMap;

use super::kubernetes_syntax::{valid_label_value, valid_qualified_name};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Labels(BTreeMap<String, String>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LabelsError {
    #[error("label key '{key}' does not use Kubernetes qualified-name syntax")]
    InvalidKey { key: String },
    #[error("label '{key}' has value '{value}' outside Kubernetes label-value syntax")]
    InvalidValue { key: String, value: String },
}

impl Labels {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, LabelsError> {
        for (key, value) in &values {
            if !valid_qualified_name(key) {
                return Err(LabelsError::InvalidKey { key: key.clone() });
            }
            if !valid_label_value(value) {
                return Err(LabelsError::InvalidValue {
                    key: key.clone(),
                    value: value.clone(),
                });
            }
        }
        Ok(Self(values))
    }

    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl TryFrom<BTreeMap<String, String>> for Labels {
    type Error = LabelsError;

    fn try_from(value: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests;
