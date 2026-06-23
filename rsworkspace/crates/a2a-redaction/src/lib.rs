//! A2A redaction primitives: typed identifiers, redactor trait, no-op
//! implementation, and Tier-3 refusal sentinel.
//!
//! Subsequent slices add the skill manifest, signed-bundle verification, and
//! the wasm redactor host on top of these foundations.
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub mod a2a_method;
pub mod error;
pub mod noop;
pub mod redactor;
pub mod signed_bundle;
pub mod skill_id;
pub mod skill_manifest;
pub mod tier3_sentinel;
pub mod wasm_bundle_path;

pub use a2a_method::{A2aMethod, ParseA2aMethodError};
pub use error::RedactionError;
pub use noop::NoopRedactor;
pub use redactor::Redactor;
pub use signed_bundle::{Ed25519PublicKey, SignatureVerificationError, SignedBundleManifest, verify_signed_bundle};
pub use skill_id::{SkillId, SkillIdError};
pub use skill_manifest::{
    JsonPathExpr, SkillCategory, SkillManifest, SkillManifestError, SkillManifestRegistry, SkillManifestVersion,
    SkillMethodMatcher, SkillSelectionPlan,
};
pub use tier3_sentinel::{TIER3_REFUSE_SENTINEL, output_is_tier3_refusal, tier3_refusal_reason_tag};
pub use wasm_bundle_path::WasmBundlePath;
