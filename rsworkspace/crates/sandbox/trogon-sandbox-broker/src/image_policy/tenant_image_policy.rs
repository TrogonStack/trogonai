use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::{OciImageRef, pattern_matches_oci_image_ref};

use super::RegistryProfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantImagePolicy {
    pub patterns: Vec<String>,
    #[serde(default)]
    pub registries: Vec<RegistryProfile>,
}

impl TenantImagePolicy {
    pub fn from_patterns(patterns: Vec<String>) -> Self {
        Self {
            patterns,
            registries: Vec::new(),
        }
    }

    /// True if `image` is granted by either a legacy pattern or a registry profile.
    pub fn matches(&self, image: &OciImageRef) -> bool {
        self.patterns.iter().any(|p| pattern_matches_oci_image_ref(p, image))
            || self.registries.iter().any(|profile| profile.matches(image))
    }
}
