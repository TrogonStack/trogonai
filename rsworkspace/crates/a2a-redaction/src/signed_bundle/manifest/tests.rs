use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::signed_bundle::Ed25519Signature;

fn skill_id(raw: &str) -> SkillId {
    SkillId::new(raw).unwrap()
}

fn signed_manifest(skill: &str) -> SignedBundleManifest {
    let sid = skill_id(skill);
    let manifest_digest = Sha256Digest::hash(br#"{"skill_id":"demo"}"#);
    let wasm_digest = Sha256Digest::hash(b"\0asm");
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let message = crate::signed_bundle::verify::sign_bundle_digest(
        SIGNED_BUNDLE_VERSION,
        &sid,
        manifest_digest,
        wasm_digest,
    );
    let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
    SignedBundleManifest::new(&sid, manifest_digest, wasm_digest, signature)
}

#[test]
fn new_sets_version_and_hex_digests() {
    let manifest = signed_manifest("demo");
    assert_eq!(manifest.version, SIGNED_BUNDLE_VERSION);
    assert_eq!(manifest.skill_id, "demo");
    assert_eq!(manifest.manifest_sha256.len(), 64);
    assert_eq!(manifest.wasm_sha256.len(), 64);
}

#[test]
fn parse_json_rejects_skill_id_mismatch() {
    let sid = skill_id("expected");
    let mut manifest = signed_manifest("other");
    manifest.skill_id = "other".into();
    let raw = serde_json::to_vec(&manifest).unwrap();

    let err = SignedBundleManifest::parse_json(&raw, &sid).unwrap_err();
    assert!(matches!(
        err,
        SignatureVerificationError::MalformedSignatureFile { detail, .. }
            if detail.contains("loader expected")
    ));
}

#[test]
fn manifest_digest_rejects_invalid_hex() {
    let mut manifest = signed_manifest("demo");
    manifest.manifest_sha256 = "zz".into();
    let err = manifest.manifest_digest(&skill_id("demo")).unwrap_err();
    assert!(matches!(
        err,
        SignatureVerificationError::MalformedSignatureFile { detail, .. }
            if detail.contains("manifest_sha256")
    ));
}

#[test]
fn signature_bytes_rejects_invalid_hex() {
    let mut manifest = signed_manifest("demo");
    manifest.signature = "00".into();
    let err = manifest.signature_bytes(&skill_id("demo")).unwrap_err();
    assert!(matches!(
        err,
        SignatureVerificationError::MalformedSignatureFile { .. }
            | SignatureVerificationError::SignatureVerificationFailed { .. }
    ));
}

#[test]
fn wasm_digest_rejects_invalid_hex() {
    let mut manifest = signed_manifest("demo");
    manifest.wasm_sha256 = "zz".into();
    let err = manifest.wasm_digest(&skill_id("demo")).unwrap_err();
    assert!(matches!(
        err,
        SignatureVerificationError::MalformedSignatureFile { detail, .. }
            if detail.contains("wasm_sha256")
    ));
}

#[test]
fn parse_json_rejects_invalid_json() {
    let err = SignedBundleManifest::parse_json(b"not-json", &skill_id("demo")).unwrap_err();
    assert!(matches!(err, SignatureVerificationError::MalformedSignatureFile { .. }));
}
