use std::collections::BTreeMap;

/// Environment overrides whose entries never appear in debug output.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct Environment(BTreeMap<String, String>);

impl Environment {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    pub fn insert(&mut self, key: String, value: String) -> Option<String> {
        self.0.insert(key, value)
    }

    pub fn remove(&mut self, key: &str) -> Option<String> {
        self.0.remove(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Environment")
            .field("len", &self.0.len())
            .finish_non_exhaustive()
    }
}

impl From<BTreeMap<String, String>> for Environment {
    fn from(value: BTreeMap<String, String>) -> Self {
        Self(value)
    }
}

impl<const N: usize> From<[(String, String); N]> for Environment {
    fn from(value: [(String, String); N]) -> Self {
        Self(BTreeMap::from(value))
    }
}

impl FromIterator<(String, String)> for Environment {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}
