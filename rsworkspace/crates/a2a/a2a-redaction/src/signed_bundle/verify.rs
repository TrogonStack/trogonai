use ed25519_dalek::Verifier;
use sha2::{Digest, Sha256};

use super::digest::Sha256Digest;
use super::error::SignatureVerificationError;
use super::manifest::{SIGNED_BUNDLE_VERSION, SignedBundleManifest};
use super::public_key::Ed25519PublicKey;
use crate::skill_id::SkillId;

/// Domain-separation tag the verifier and signer agree on. Including the tag
/// in the signed message prevents a confused-deputy attack where a signature
/// produced for some other Ed25519 message could be replayed here.
const SIGNED_BUNDLE_SIGNATURE_DOMAIN: &[u8] = b"a2a-redaction/signed-bundle/v1";

/// Construct the canonical signed message for a bundle.
///
/// The message binds the bundle version, skill id, manifest digest, and wasm
/// digest together so that swapping the envelope's `skill_id` (or `version`)
/// while keeping the same manifest+wasm digests invalidates the signature.
/// Layout:
///
/// ```text
/// SHA256(
///   domain_tag
///   || u32_be(version)
///   || u32_be(skill_id.len()) || skill_id_bytes
///   || manifest_digest (32 bytes)
///   || wasm_digest (32 bytes)
/// )
/// ```
pub fn sign_bundle_digest(
    version: u32,
    skill_id: &SkillId,
    manifest_digest: Sha256Digest,
    wasm_digest: Sha256Digest,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SIGNED_BUNDLE_SIGNATURE_DOMAIN);
    hasher.update(version.to_be_bytes());
    let skill_bytes = skill_id.as_str().as_bytes();
    hasher.update((skill_bytes.len() as u32).to_be_bytes());
    hasher.update(skill_bytes);
    hasher.update(manifest_digest.as_bytes());
    hasher.update(wasm_digest.as_bytes());
    let out = hasher.finalize();
    let mut message = [0u8; 32];
    message.copy_from_slice(&out);
    message
}

pub fn verify_signed_bundle(
    pubkey: &Ed25519PublicKey,
    manifest_bytes: &[u8],
    wasm_bytes: &[u8],
    envelope: &SignedBundleManifest,
) -> Result<(), SignatureVerificationError> {
    let skill_id =
        SkillId::new(envelope.skill_id.clone()).map_err(|e| SignatureVerificationError::MalformedSignatureFile {
            skill_id: envelope.skill_id.clone(),
            detail: format!("invalid skill_id: {e}"),
        })?;

    if envelope.version != SIGNED_BUNDLE_VERSION {
        return Err(SignatureVerificationError::MalformedSignatureFile {
            skill_id: skill_id.to_string(),
            detail: format!("unsupported version {}", envelope.version),
        });
    }

    let expected_manifest = Sha256Digest::hash(manifest_bytes);
    let expected_wasm = Sha256Digest::hash(wasm_bytes);
    let envelope_manifest = envelope.manifest_digest(&skill_id)?;
    let envelope_wasm = envelope.wasm_digest(&skill_id)?;

    if expected_manifest != envelope_manifest {
        return Err(SignatureVerificationError::ManifestSha256Mismatch {
            skill_id: skill_id.to_string(),
        });
    }
    if expected_wasm != envelope_wasm {
        return Err(SignatureVerificationError::WasmSha256Mismatch {
            skill_id: skill_id.to_string(),
        });
    }

    let verifying_key =
        pubkey
            .verifying_key()
            .map_err(|_| SignatureVerificationError::SignatureVerificationFailed {
                skill_id: skill_id.to_string(),
            })?;
    let signature = envelope.signature_bytes(&skill_id)?;
    let message = sign_bundle_digest(envelope.version, &skill_id, expected_manifest, expected_wasm);

    verifying_key
        .verify(&message, &signature.dalek_signature()?)
        .map_err(|_| SignatureVerificationError::SignatureVerificationFailed {
            skill_id: skill_id.to_string(),
        })
}

#[cfg(test)]
mod tests;
