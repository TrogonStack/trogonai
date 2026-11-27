use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{OciImageRef, pattern_matches_legacy};

use super::RegistryKind;

/// Grants a tenant access to a whole registry, optionally narrowed to
/// repositories matching `repository_patterns` (same glob semantics as the
/// legacy `TenantImagePolicy::patterns`, applied to the repository alone).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryProfile {
    pub kind: RegistryKind,
    #[serde(default)]
    pub repository_patterns: Vec<String>,
}

impl RegistryProfile {
    pub fn matches(&self, image: &OciImageRef) -> bool {
        self.kind.matches_registry(&image.registry)
            && (self.repository_patterns.is_empty()
                || self
                    .repository_patterns
                    .iter()
                    .any(|pattern| pattern_matches_legacy(pattern, &image.repository)))
    }
}
