use crate::ConfigValue;
use std::collections::BTreeMap;

/// Environment overrides whose names appear in debug output while sensitive
/// values stay redacted.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Environment(BTreeMap<String, ConfigValue>);

impl Environment {
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        self.0.get(key)
    }

    pub fn insert(&mut self, key: String, value: ConfigValue) -> Option<ConfigValue> {
        self.0.insert(key, value)
    }

    pub fn remove(&mut self, key: &str) -> Option<ConfigValue> {
        self.0.remove(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &ConfigValue)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }
}

impl From<BTreeMap<String, ConfigValue>> for Environment {
    fn from(value: BTreeMap<String, ConfigValue>) -> Self {
        Self(value)
    }
}

impl<const N: usize> From<[(String, ConfigValue); N]> for Environment {
    fn from(value: [(String, ConfigValue); N]) -> Self {
        Self(BTreeMap::from(value))
    }
}

impl FromIterator<(String, ConfigValue)> for Environment {
    fn from_iter<I: IntoIterator<Item = (String, ConfigValue)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}
