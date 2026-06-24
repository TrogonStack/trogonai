use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::signed_bundle::Ed25519Signature;

fn fixture_keypair() -> (Ed25519PublicKey, SigningKey) {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let verifying_key = signing_key.verifying_key();
    (Ed25519PublicKey::from_bytes(*verifying_key.as_bytes()), signing_key)
}

fn signed_envelope(
    skill_id: &str,
    manifest_bytes: &[u8],
    wasm_bytes: &[u8],
    signing_key: &SigningKey,
) -> SignedBundleManifest {
    let sid = SkillId::new(skill_id).expect("test fixture skill id is valid");
    let manifest_digest = Sha256Digest::hash(manifest_bytes);
    let wasm_digest = Sha256Digest::hash(wasm_bytes);
    let message = sign_bundle_digest(SIGNED_BUNDLE_VERSION, &sid, manifest_digest, wasm_digest);
    let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
    SignedBundleManifest::new(&sid, manifest_digest, wasm_digest, signature)
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
fn swapping_envelope_skill_id_invalidates_signature() {
    // Regression: the signed message now binds skill_id, so re-stamping
    // the envelope with a different (valid) skill id must fail
    // verification even though manifest+wasm digests still match.
    let (pubkey, signing_key) = fixture_keypair();
    let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
    let wasm = b"\0asm";
    let mut envelope = signed_envelope("demo", manifest, wasm, &signing_key);
    envelope.skill_id = "other-skill".into();
    let err = verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect_err("must reject");
    assert!(matches!(
        err,
        SignatureVerificationError::SignatureVerificationFailed { .. }
    ));
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

#[test]
fn malformed_envelope_skill_id() {
    let (pubkey, signing_key) = fixture_keypair();
    let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
    let wasm = b"\0asm";
    let mut envelope = signed_envelope("demo", manifest, wasm, &signing_key);
    envelope.skill_id = "../bad".into();
    let err = verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect_err("bad skill id");
    assert!(matches!(
        err,
        SignatureVerificationError::MalformedSignatureFile { detail, .. }
            if detail.contains("invalid skill_id")
    ));
}

#[test]
fn invalid_public_key_fails_verification() {
    let pubkey = Ed25519PublicKey::from_bytes([0u8; 32]);
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let manifest = br#"{"skill_id":"demo","json_path":"$.x"}"#;
    let wasm = b"\0asm";
    let envelope = signed_envelope("demo", manifest, wasm, &signing_key);
    let err = verify_signed_bundle(&pubkey, manifest, wasm, &envelope).expect_err("invalid pubkey");
    assert!(matches!(
        err,
        SignatureVerificationError::SignatureVerificationFailed { .. }
    ));
}
