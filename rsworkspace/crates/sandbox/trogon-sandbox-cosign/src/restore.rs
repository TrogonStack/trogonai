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

/// Verify the signature of a snapshot being restored, binding `expected_env_digest` (the digest
/// recorded in the snapshot itself) to the signed payload when there is one.
pub async fn enforce_restore_signature(
    mode: VerifyMode,
    verifier: Option<&CosignVerifier>,
    reference: &OciArtifactRef,
    expected_env_digest: Option<&EnvDigest>,
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
            if let Some(digest) = expected_env_digest {
                required.insert(EnvDigest::ANNOTATION_KEY.to_string(), digest.as_str().to_string());
            }
            verify_snapshot_signature_with_annotations(verifier, reference, &required).await
        }
    };
    apply_mode(mode, outcome, reference)
}
