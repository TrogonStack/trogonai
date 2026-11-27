//! Cosign signing and verification for sandbox snapshot OCI artifacts.
//!
//! Signing uses KMS-backed keys through the `cosign` CLI, or an in-process local key. Verification
//! checks the signature layers published next to a snapshot against a configured PEM public key.

mod attach;
mod constants;
mod cosign_error;
mod cosign_signer;
mod cosign_verifier;
mod failure;
mod restore;
mod snapshot_pull_error;
mod test_key_pair;
mod verify;
mod verify_mode;
mod verify_mode_error;

pub use attach::{attach_cosign_signature, attach_cosign_signature_with_annotations};
pub use constants::COSIGN_SIMPLE_SIGNING_MEDIA_TYPE;
pub use cosign_error::CosignError;
pub use cosign_signer::CosignSigner;
pub use cosign_verifier::CosignVerifier;
pub use restore::{apply_mode, enforce_restore_signature};
pub use snapshot_pull_error::{SnapshotPullError, into_pull_error, snapshot_signature_lease_error};
pub use test_key_pair::generate_test_key_pair;
pub use verify::{verify_if_configured, verify_snapshot_signature, verify_snapshot_signature_with_annotations};
pub use verify_mode::VerifyMode;
pub use verify_mode_error::VerifyModeError;
