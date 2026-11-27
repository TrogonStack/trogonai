//! Durable manifests for the native snapshots this worker captured, and the identities they bind.

mod binding;
mod build_id;
mod build_id_error;
mod captured_deadline;
mod captured_deadline_error;
mod captured_execution;
mod constants;
mod logical_name;
mod logical_name_error;
mod manifest;
mod manifest_error;
mod manifests;
mod manifests_error;
mod opaque_id;
mod process_identity;
mod process_identity_error;
mod record_outcome;
mod reference;
mod reference_error;
mod template_id;
mod template_id_error;
#[cfg(test)]
mod tests;

pub(crate) use binding::SnapshotBinding;
pub(crate) use build_id::SnapshotBuildId;
pub(crate) use captured_deadline::CapturedDeadline;
pub(crate) use captured_execution::CapturedExecution;
pub(crate) use constants::MAX_EXECUTIONS;
pub(crate) use logical_name::validate_logical_name;
pub(crate) use manifest::SnapshotManifest;
pub use manifest_error::SnapshotManifestError;
pub(crate) use manifests::SnapshotManifests;
pub use manifests_error::SnapshotManifestsError;
pub(crate) use opaque_id::is_opaque_id;
pub(crate) use process_identity::ProcessIdentity;
pub(crate) use record_outcome::RecordOutcome;
pub(crate) use reference::{validate_reference, validate_reference_identity};
pub use reference_error::SnapshotReferenceError;
pub(crate) use template_id::SnapshotTemplateId;
