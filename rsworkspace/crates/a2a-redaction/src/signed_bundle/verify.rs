use ed25519_dalek::Verifier;

use super::digest::Sha256Digest;
use super::error::SignatureVerificationError;
use super::manifest::{SignedBundleManifest, SIGNED_BUNDLE_VERSION};
use super::public_key::Ed25519PublicKey;
use crate::skill_id::SkillId;

pub fn sign_bundle_digest(manifest_digest: Sha256Digest, wasm_digest: Sha256Digest) -> [u8; 64] {
    let mut message = [0u8; 64];
    message[..32].copy_from_slice(manifest_digest.as_bytes());
    message[32..].copy_from_slice(wasm_digest.as_bytes());
    message
}

pub fn verify_signed_bundle(
    pubkey: &Ed25519PublicKey,
    manifest_bytes: &[u8],
    wasm_bytes: &[u8],
    envelope: &SignedBundleManifest,
) -> Result<(), SignatureVerificationError> {
    let skill_id = SkillId::new(envelope.skill_id.clone());

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

    let verifying_key = pubkey.verifying_key().map_err(|_| SignatureVerificationError::SignatureVerificationFailed {
        skill_id: skill_id.to_string(),
    })?;
    let signature = envelope.signature_bytes(&skill_id)?;
    let message = sign_bundle_digest(expected_manifest, expected_wasm);

    verifying_key
        .verify(&message, &signature.dalek_signature()?)
        .map_err(|_| SignatureVerificationError::SignatureVerificationFailed {
            skill_id: skill_id.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};

    use crate::signed_bundle::Ed25519Signature;
    use super::*;

    fn fixture_keypair() -> (Ed25519PublicKey, SigningKey) {
        let signing_key = SigningKey::from_bytes(&[7u8; 32]);
        let verifying_key = signing_key.verifying_key();
        (
            Ed25519PublicKey::from_bytes(*verifying_key.as_bytes()),
            signing_key,
        )
    }

    fn signed_envelope(
        skill_id: &str,
        manifest_bytes: &[u8],
        wasm_bytes: &[u8],
        signing_key: &SigningKey,
    ) -> SignedBundleManifest {
        let manifest_digest = Sha256Digest::hash(manifest_bytes);
        let wasm_digest = Sha256Digest::hash(wasm_bytes);
        let message = sign_bundle_digest(manifest_digest, wasm_digest);
        let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
        SignedBundleManifest::new(&SkillId::new(skill_id), manifest_digest, wasm_digest, signature)
    }

    #[test]
    fn verify_happy_path() {
        let (pubkey, signing_key) = fixture_keypair();
        let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
        let wasm = b"\0asm";
        let envelope = signed_envelope("demo", manifest, wasm, &signing_key);
        verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect("valid bundle");
    }

    #[test]
    fn manifest_sha256_mismatch() {
        let (pubkey, signing_key) = fixture_keypair();
        let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
        let wasm = b"\0asm";
        let envelope = signed_envelope("demo", manifest, wasm, &signing_key);
        let err = verify_signed_bundle(&pubkey, b"tampered", wasm, &envelope).expect_err("mismatch");
        assert!(matches!(err, SignatureVerificationError::ManifestSha256Mismatch { .. }));
    }

    #[test]
    fn wasm_sha256_mismatch() {
        let (pubkey, signing_key) = fixture_keypair();
        let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
        let wasm = b"\0asm";
        let envelope = signed_envelope("demo", manifest, wasm, &signing_key);
        let err = verify_signed_bundle(&pubkey, manifest, b"tampered", &envelope).expect_err("mismatch");
        assert!(matches!(err, SignatureVerificationError::WasmSha256Mismatch { .. }));
    }

    #[test]
    fn signature_verification_failure() {
        let (pubkey, signing_key) = fixture_keypair();
        let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
        let wasm = b"\0asm";
        let mut envelope = signed_envelope("demo", manifest, wasm, &signing_key);
        envelope.signature = "00".repeat(64);
        let err = verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect_err("bad sig");
        assert!(matches!(
            err,
            SignatureVerificationError::SignatureVerificationFailed { .. }
                | SignatureVerificationError::MalformedSignatureFile { .. }
        ));
    }

    #[test]
    fn malformed_envelope_version() {
        let (pubkey, signing_key) = fixture_keypair();
        let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
        let wasm = b"\0asm";
        let mut envelope = signed_envelope("demo", manifest, wasm, &signing_key);
        envelope.version = 99;
        let err = verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect_err("bad version");
        assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
    }

    #[test]
    fn malformed_envelope_digest_hex() {
        let (pubkey, signing_key) = fixture_keypair();
        let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
        let wasm = b"\0asm";
        let mut envelope = signed_envelope("demo", manifest, wasm, &signing_key);
        envelope.manifest_sha256 = "zz".into();
        let err = verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect_err("bad hex");
        assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
    }
}
