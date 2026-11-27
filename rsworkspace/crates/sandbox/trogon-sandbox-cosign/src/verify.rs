use std::collections::BTreeMap;

use sigstore::cosign::verification_constraint::{AnnotationVerifier, PublicKeyVerifier, VerificationConstraintVec};
use sigstore::cosign::{self, CosignCapabilities, verify_constraints};
use sigstore::crypto::SigningScheme;
use sigstore::registry::{Auth, ClientConfig, ClientProtocol};
use trogon_sandbox_contract::OciArtifactRef;

use crate::failure::{public_key_failed, verify_failed};
use crate::{CosignError, CosignVerifier};

/// Verify a snapshot signature when a verifier is configured; no-op when unset.
pub async fn verify_if_configured(
    verifier: Option<&CosignVerifier>,
    reference: &OciArtifactRef,
) -> Result<(), CosignError> {
    match verifier {
        Some(verifier) => verify_snapshot_signature(verifier, reference).await,
        None => Ok(()),
    }
}

pub async fn verify_snapshot_signature(
    verifier: &CosignVerifier,
    reference: &OciArtifactRef,
) -> Result<(), CosignError> {
    verify_snapshot_signature_with_annotations(verifier, reference, &BTreeMap::new()).await
}

/// Like [`verify_snapshot_signature`], and also requires that the same signature carries every
/// entry of `required` in its signed payload (the `optional` map of the simple-signing
/// document). A valid signature whose payload lacks an entry or names another value fails with
/// [`CosignError::AnnotationMismatch`], so a registry cannot relabel a signed snapshot.
pub async fn verify_snapshot_signature_with_annotations(
    verifier: &CosignVerifier,
    reference: &OciArtifactRef,
    required: &BTreeMap<String, String>,
) -> Result<(), CosignError> {
    let image_ref = reference
        .allowlist_string()
        .parse()
        .map_err(verify_failed("invalid OCI reference"))?;

    let mut oci_client_config = ClientConfig::default();
    if !verifier.insecure_registries.is_empty() {
        oci_client_config.protocol = ClientProtocol::HttpsExcept(verifier.insecure_registries.clone());
    }

    let mut client = cosign::ClientBuilder::default()
        .with_oci_client_config(oci_client_config)
        .build()
        .map_err(verify_failed("build cosign client"))?;

    let auth = Auth::Anonymous;
    let signature_layers = client
        .trusted_signature_layers(&auth, &image_ref)
        .await
        .map_err(verify_failed("fetch signature layers"))?;

    if signature_layers.is_empty() {
        return Err(CosignError::SignatureInvalid {
            digest: reference.digest.clone(),
        });
    }

    let key_only: VerificationConstraintVec = vec![Box::new(public_key_verifier(verifier)?)];
    if verify_constraints(&signature_layers, key_only.iter()).is_err() {
        return Err(CosignError::SignatureInvalid {
            digest: reference.digest.clone(),
        });
    }
    if required.is_empty() {
        return Ok(());
    }

    // The key check passed on some layer; now one layer must pass the key and the annotations
    // together, so a signature over other content cannot lend its key to a relabeled payload.
    let both: VerificationConstraintVec = vec![
        Box::new(public_key_verifier(verifier)?),
        Box::new(AnnotationVerifier {
            annotations: required.clone(),
        }),
    ];
    verify_constraints(&signature_layers, both.iter()).map_err(|_| CosignError::AnnotationMismatch {
        digest: reference.digest.clone(),
    })
}

fn public_key_verifier(verifier: &CosignVerifier) -> Result<PublicKeyVerifier, CosignError> {
    PublicKeyVerifier::new(&verifier.public_key_pem, &SigningScheme::default())
        .map_err(public_key_failed("parse public key"))
}
