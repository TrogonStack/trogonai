use std::collections::BTreeMap;

use super::kubernetes_syntax::valid_qualified_name;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Annotations(BTreeMap<String, String>);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("annotation key '{key}' does not use Kubernetes qualified-name syntax")]
pub struct AnnotationsError {
    key: String,
}

impl Annotations {
    pub fn new(values: BTreeMap<String, String>) -> Result<Self, AnnotationsError> {
        for key in values.keys() {
            if !valid_qualified_name(key) {
                return Err(AnnotationsError { key: key.clone() });
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

impl TryFrom<BTreeMap<String, String>> for Annotations {
    type Error = AnnotationsError;

    fn try_from(value: BTreeMap<String, String>) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests;
