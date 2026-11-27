//! Per-tenant image allow-lists.

mod image_allow_list;
mod in_memory_image_allow_list;
mod nats_kv_image_allow_list;
mod registry_kind;
mod registry_profile;
mod tenant_image_policy;

use trogon_sandbox_contract::{OciImageRef, pattern_matches_legacy, pattern_matches_oci_image_ref};

pub use image_allow_list::ImageAllowList;
pub use in_memory_image_allow_list::InMemoryImageAllowList;
pub use nats_kv_image_allow_list::NatsKvImageAllowList;
pub use registry_kind::RegistryKind;
pub use registry_profile::RegistryProfile;
pub use tenant_image_policy::TenantImagePolicy;

pub fn pattern_matches(pattern: &str, image: &str) -> bool {
    pattern_matches_legacy(pattern, image)
}

pub fn pattern_matches_image(pattern: &str, image: &OciImageRef) -> bool {
    pattern_matches_oci_image_ref(pattern, image)
}

#[cfg(test)]
mod tests;
