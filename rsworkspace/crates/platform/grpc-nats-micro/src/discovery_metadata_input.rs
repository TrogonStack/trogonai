//! Discovery metadata exactly as an annotation spelled it (ADR 0016 §1).

use std::collections::HashMap;

/// Untrusted discovery metadata, as it arrives in
/// `trogon.nats.micro.v1alpha1.ServiceOptions.metadata` or `MethodOptions.metadata`.
/// Carries no guarantee that the entries can address anything in a discovery
/// record; [`crate::DiscoveryMetadata::from_input`] is the single conversion
/// into the domain value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryMetadataInput(HashMap<String, String>);

impl DiscoveryMetadataInput {
    pub fn new<I, K, V>(entries: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value.into()))
                .collect(),
        )
    }

    pub const fn entries(&self) -> &HashMap<String, String> {
        &self.0
    }
}
