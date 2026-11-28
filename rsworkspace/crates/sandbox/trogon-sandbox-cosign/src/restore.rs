use std::collections::BTreeMap;

use trogon_sandbox_contract::{EnvDigest, OciArtifactRef};

use crate::verify::verify_snapshot_signature_with_annotations;
use crate::{CosignError, CosignVerifier, VerifyMode};

/// Apply `mode` to a verification outcome: `Require` passes the error through, `Warn` logs it
/// and continues, `Off` ignores it.
pub fn apply_mode(
    mode: VerifyMode,
    outcome: Result<(), CosignError>,
    reference: &OciArtifactRef,
) -> Result<(), CosignError> {
    match (mode, outcome) {
        (_, Ok(())) | (VerifyMode::Off, _) => Ok(()),
        (VerifyMode::Warn, Err(error)) => {
            tracing::warn!(
                digest = %reference.digest,
                %error,
                "snapshot signature did not verify; restoring anyway because cosign verification is in warn mode"
            );
            Ok(())
        }
        (VerifyMode::Require, Err(error)) => Err(error),
    }
}

/// Whether the signed payload of a restored snapshot must carry its recorded environment digest.
/// The caller states which, so skipping the binding is never an accident of a missing value.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EnvDigestBinding<'a> {
    /// The signed payload must carry exactly this digest.
    Bound(&'a EnvDigest),
    /// The snapshot records no environment digest, so only the signature itself is checked.
    Unbound,
}

impl<'a> EnvDigestBinding<'a> {
    /// `Bound` when the snapshot recorded a digest, otherwise an explicit `Unbound`.
    pub fn from_recorded(recorded: Option<&'a EnvDigest>) -> Self {
        recorded.map_or(Self::Unbound, Self::Bound)
    }
}

/// Verify the signature of a snapshot being restored, binding the environment digest recorded
/// in the snapshot itself to the signed payload when `binding` says so.
pub async fn enforce_restore_signature(
    mode: VerifyMode,
    verifier: Option<&CosignVerifier>,
    reference: &OciArtifactRef,
    binding: EnvDigestBinding<'_>,
) -> Result<(), CosignError> {
    if mode == VerifyMode::Off {
        return Ok(());
    }
    let outcome = match verifier {
        None => Err(CosignError::PublicKey(
            "signature verification is enabled but no public key is configured".into(),
        )),
        Some(verifier) => {
            let mut required = BTreeMap::new();
            if let EnvDigestBinding::Bound(digest) = binding {
                required.insert(EnvDigest::ANNOTATION_KEY.to_string(), digest.as_str().to_string());
            }
            verify_snapshot_signature_with_annotations(verifier, reference, &required).await
        }
    };
    apply_mode(mode, outcome, reference)
}
