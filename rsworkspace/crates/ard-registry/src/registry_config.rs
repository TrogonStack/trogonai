//! Registry runtime configuration.

use ard_catalog::{CatalogEntry, CatalogManifest};

use crate::source_url::SourceUrl;

/// Configuration for an in-memory ARD registry runtime.
#[derive(Clone, Debug)]
pub struct RegistryConfig {
    source_url: SourceUrl,
    manifest: CatalogManifest,
    referrals: Vec<CatalogEntry>,
}

impl RegistryConfig {
    pub fn new(source_url: SourceUrl, manifest: CatalogManifest, referrals: Vec<CatalogEntry>) -> Self {
        Self {
            source_url,
            manifest,
            referrals,
        }
    }

    pub fn source_url(&self) -> &str {
        self.source_url.as_str()
    }

    pub fn manifest(&self) -> &CatalogManifest {
        &self.manifest
    }

    pub fn referrals(&self) -> &[CatalogEntry] {
        &self.referrals
    }
}
