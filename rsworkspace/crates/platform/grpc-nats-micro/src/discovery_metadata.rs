//! The metadata map that populates one service's or one endpoint's NATS
//! Services discovery record (ADR 0016 §1).

use std::collections::HashMap;

use crate::discovery_metadata_input::DiscoveryMetadataInput;

/// Why a [`DiscoveryMetadata`] could not be constructed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DiscoveryMetadataError {
    #[error("discovery metadata key must not be empty")]
    EmptyKey,
}

/// Metadata `$SRV.INFO` reports for a service or one of its endpoints.
///
/// NATS Services (ADR-32) leaves the map opaque, so the only thing to
/// guarantee is that every entry is addressable: a nameless key is not
/// something a discovery consumer can ask for, and it would otherwise reach
/// `$SRV.INFO` as a silent `"": ...` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiscoveryMetadata(HashMap<String, String>);

impl DiscoveryMetadata {
    pub fn from_input(input: &DiscoveryMetadataInput) -> Result<Self, DiscoveryMetadataError> {
        if input.entries().keys().any(|key| key.is_empty()) {
            return Err(DiscoveryMetadataError::EmptyKey);
        }
        Ok(Self(input.entries().clone()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub const fn entries(&self) -> &HashMap<String, String> {
        &self.0
    }
}

#[cfg(test)]
mod tests;
