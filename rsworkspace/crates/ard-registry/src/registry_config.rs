//! Registry runtime configuration.

use ard_catalog::{CatalogEntry, CatalogManifest};

/// Configuration for an in-memory ARD registry runtime.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    source_url: String,
    manifest: CatalogManifest,
    referrals: Vec<CatalogEntry>,
}

impl RegistryConfig {
    pub fn new(source_url: impl Into<String>, manifest: CatalogManifest, referrals: Vec<CatalogEntry>) -> Self {
        Self {
            source_url: source_url.into(),
            manifest,
            referrals,
        }
    }

    pub fn source_url(&self) -> &str {
        &self.source_url
    }

    pub fn manifest(&self) -> &CatalogManifest {
        &self.manifest
    }

    pub fn referrals(&self) -> &[CatalogEntry] {
        &self.referrals
    }
}
