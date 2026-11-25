use serde::{Deserialize, Serialize};

/// PEM encoded CA certificates trusted in addition to the system roots when the proxy verifies an upstream it
/// re-encrypts to.
///
/// It serializes as a JSON array of strings, one PEM bundle each.
#[derive(Debug, Clone, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UpstreamCaCertificates(Vec<String>);

impl UpstreamCaCertificates {
    pub fn new(pem_bundles: impl IntoIterator<Item = String>) -> Self {
        Self(pem_bundles.into_iter().collect())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, String> {
        self.0.iter()
    }
}
