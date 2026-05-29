//! Build the canonical first-party MCP policy bundle (`global/mcp-pack`).
//!
//! Output is an unsigned tar archive of members plus NKey-signed `manifest.toml`, consumable by
//! [`trogon_mcp_gateway::bundle::load_bundle`].

mod build;
mod cel;
mod components;
mod sign;

use std::fmt;
use std::path::Path;

use nkeys::KeyPair;
use trogon_mcp_gateway::bundle::BundleLoadError;

pub use build::{assemble_archive, assemble_manifest, default_members, BundleMember};
pub use cel::{
    AUDIT_ID, AUDIT_PATH, CATALOG_FILTER_ID, CATALOG_FILTER_PATH, DEFAULT_AUDIT,
    DEFAULT_CATALOG_FILTER, DEFAULT_RESOURCE_TUPLE, RESOURCE_TUPLE_ID, RESOURCE_TUPLE_PATH,
};
pub use components::{SCHEMA_LEARNER_ID, SCHEMA_LEARNER_PATH, SCHEMA_LEARNER_STUB};

/// Default manifest `name` (`global/mcp-pack`): ADR 0028 `_global/mcp-pack` with `_` omitted because
/// the gateway scope validator allows only `[a-z0-9-]` in tenant/slug segments today.
pub const DEFAULT_PACK_NAME: &str = "global/mcp-pack";
pub const DEFAULT_PACK_VERSION: &str = "0.1.0";

#[derive(Debug, Clone)]
pub struct McpPackSpec {
    pub name: String,
    pub version: String,
    pub min_gateway_version: String,
    pub cel_version: String,
    pub author: String,
    pub created_at: String,
    pub description: String,
    pub signer: KeyPair,
    pub include_schema_learner: bool,
}

impl McpPackSpec {
    pub fn with_signer(signer: KeyPair) -> Self {
        Self {
            name: DEFAULT_PACK_NAME.into(),
            version: DEFAULT_PACK_VERSION.into(),
            min_gateway_version: trogon_mcp_gateway::bundle::GATEWAY_VERSION.into(),
            cel_version: "0.10".into(),
            author: "trogonstack/platform".into(),
            created_at: "2026-05-29T00:00:00Z".into(),
            description: "First-party MCP pack: catalog shaping, resource-tuple derivation, audit enrich, schema-learner stub".into(),
            signer,
            include_schema_learner: true,
        }
    }

    pub fn with_ephemeral_signer() -> Self {
        Self::with_signer(KeyPair::new_user())
    }
}

pub struct McpPack {
    spec: McpPackSpec,
}

impl McpPack {
    pub fn new(spec: McpPackSpec) -> Self {
        Self { spec }
    }

    pub fn spec(&self) -> &McpPackSpec {
        &self.spec
    }

    pub fn build(&self) -> Result<Vec<u8>, McpPackError> {
        build::assemble_archive(&self.spec)
    }

    pub fn build_to_path(&self, path: impl AsRef<Path>) -> Result<(), McpPackError> {
        let bytes = self.build()?;
        std::fs::write(path, bytes).map_err(|error| McpPackError::Io(error.to_string()))?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum McpPackError {
    Build(String),
    Manifest(BundleLoadError),
    Sign(String),
    Io(String),
}

impl fmt::Display for McpPackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Build(message) => write!(f, "mcp-pack build failed: {message}"),
            Self::Manifest(error) => write!(f, "manifest error: {error}"),
            Self::Sign(message) => write!(f, "signing failed: {message}"),
            Self::Io(message) => write!(f, "io error: {message}"),
        }
    }
}

impl std::error::Error for McpPackError {}

impl From<BundleLoadError> for McpPackError {
    fn from(error: BundleLoadError) -> Self {
        Self::Manifest(error)
    }
}

#[cfg(test)]
mod tests {
    use trogon_mcp_gateway::bundle::{load_bundle, TrustedKeys};

    use super::*;

    #[test]
    fn build_output_loads_via_gateway_loader() {
        let spec = McpPackSpec::with_ephemeral_signer();
        let trusted = TrustedKeys::from_allowlist([spec.signer.public_key()]);
        let pack = McpPack::new(spec);
        let archive = pack.build().expect("assemble bundle");
        let loaded = load_bundle(&archive, &trusted).expect("load signed bundle");
        assert_eq!(loaded.manifest.name, DEFAULT_PACK_NAME);
        assert_eq!(loaded.manifest.version, DEFAULT_PACK_VERSION);
        assert_eq!(loaded.programs.len(), 3);
        assert_eq!(loaded.components.len(), 1);
        assert_eq!(loaded.components[0].entry.id, SCHEMA_LEARNER_ID);
    }

    #[test]
    fn cel_only_bundle_without_schema_learner() {
        let mut spec = McpPackSpec::with_ephemeral_signer();
        spec.include_schema_learner = false;
        let trusted = TrustedKeys::from_allowlist([spec.signer.public_key()]);
        let loaded = load_bundle(&McpPack::new(spec).build().expect("build"), &trusted)
            .expect("load");
        assert!(loaded.components.is_empty());
        assert_eq!(loaded.programs.len(), 3);
    }
}
