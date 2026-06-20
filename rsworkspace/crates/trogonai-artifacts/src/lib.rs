//! Session artifact store: inline vs claim-check persistence and kernel integration.

pub mod browser;
pub mod config;
pub mod error;
pub mod image;
pub mod kernel;
pub mod maintenance;
pub mod nats;
pub mod preview;
pub mod provision;
pub mod redaction;
pub mod store;
pub mod telemetry;

mod checksum;

pub use browser::{ArtifactBrowse, ArtifactBrowseEntry, browse_artifacts};
pub use config::{ArtifactStoreConfig, DEFAULT_PREVIEW_MAX_BYTES, session_artifacts_bucket};
pub use error::ArtifactStoreError;
pub use image::{
    FetchLimits, FetchedImage, ImageFetchError, ImageFetcher, UrlImageOutcome, resolve_url_image,
};
pub use kernel::{
    build_artifact_created_event, gc_unreferenced_artifacts, record_external_ref_image,
    store_and_emit_artifact_created, ArtifactEventContext,
};
pub use maintenance::{SessionMaintenanceReport, run_session_maintenance};
pub use nats::{artifact_object_key, artifact_storage_ref, object_key_from_storage_ref};
pub use preview::build_preview;
pub use provision::provision_artifact_object_store;
pub use redaction::{redact_secrets, RedactionOutcome};
pub use store::{
    ArtifactAvailability, ArtifactStorageMode, ArtifactStore, ArtifactUnavailable,
    ArtifactUnavailableReason, RetrievedArtifact, StoreArtifactRequest, StoredArtifact,
};

pub use checksum::sha256_hex;
