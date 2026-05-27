mod audit;
mod config;
mod kv;
mod manifest;
mod run;
mod signer;
mod sync;

pub use audit::{AuditEvent, AuditPublisher, RegistryMutationKind};
pub use config::ControllerConfig;
pub use kv::{ControllerStore, SyncOutcome, connect_nats};
pub use manifest::{
    ManifestError, ManifestRecord, ManifestSignature, SignedManifest, ValidationError, MANIFEST_VERSION,
    SIGNATURE_ALGORITHM, digest_format, monotonic_version, signing_payload, validate_agent_id,
    validate_manifest_record, verify_manifest_signature,
};
pub use run::{run_controller, validate_repo_sync};
pub use signer::{FileManifestSigner, KmsManifestSigner, ManifestSigner, SignerError, SignerKind, load_signer, verifying_key_from_pem};
pub use sync::{GitSyncConfig, SyncEngine, SyncError, SyncReport, discover_manifest_paths, pull_latest_commit};
