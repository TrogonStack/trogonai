use std::fs;
use std::path::Path;

use ed25519_dalek::{Signer, SigningKey};

use super::*;
use crate::signed_bundle::{Ed25519Signature, Sha256Digest, sign_bundle_digest};
use a2a::types::{PartContent, Role};

fn fixture_path() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/identity_redact_part.wasm"
    ))
}

fn write_signed_bundle(dir: &Path, skill: &str, signing_key: &SigningKey, manifest_bytes: &[u8], wasm_bytes: &[u8]) {
    fs::write(dir.join(format!("{skill}.wasm")), wasm_bytes).expect("write wasm");
    fs::write(dir.join(format!("{skill}.manifest.json")), manifest_bytes).expect("write manifest");
    let sid = SkillId::new(skill).expect("valid");
    let manifest_digest = Sha256Digest::hash(manifest_bytes);
    let wasm_digest = Sha256Digest::hash(wasm_bytes);
    let message = sign_bundle_digest(
        crate::signed_bundle::SIGNED_BUNDLE_VERSION,
        &sid,
        manifest_digest,
        wasm_digest,
    );
    let signature = Ed25519Signature::from_bytes(signing_key.sign(&message).to_bytes());
    let envelope = SignedBundleManifest::new(&sid, manifest_digest, wasm_digest, signature);
    let sig_json = serde_json::to_vec_pretty(&envelope).expect("serialize sig");
    fs::write(dir.join(format!("{skill}.sig")), sig_json).expect("write sig");
}

#[test]
fn passthrough_when_no_registered_module_for_message() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let host = WasmRedactorHost::new(dir).unwrap();
    let msg = Message {
        message_id: "m".into(),
        context_id: None,
        task_id: None,
        role: Role::Agent,
        parts: vec![],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let out = host
        .redact_message(msg.clone(), &SkillId::new("missing").expect("valid"))
        .unwrap();
    assert_eq!(serde_json::to_value(out).unwrap(), serde_json::to_value(msg).unwrap());
}

#[test]
fn passthrough_when_no_registered_module_for_artifact() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let host = WasmRedactorHost::new(dir).unwrap();
    let art = Artifact {
        artifact_id: "aid".into(),
        name: None,
        description: None,
        parts: vec![],
        metadata: None,
        extensions: None,
    };
    let out = host
        .redact_artifact(art.clone(), &SkillId::new("missing").expect("valid"))
        .unwrap();
    assert_eq!(serde_json::to_value(out).unwrap(), serde_json::to_value(art).unwrap());
}

#[test]
fn wasm_skill_dispatches_through_message_parts() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let host = WasmRedactorHost::new(dir).unwrap();
    let wasm = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/identity_redact_part.wasm"
    ));
    let skill = SkillId::new("fixture").expect("valid");
    host.register_skill_wasm(skill.clone(), wasm).unwrap();

    let msg_in = Message {
        message_id: "m".into(),
        context_id: None,
        task_id: None,
        role: Role::Agent,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("x".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let got = host.redact_message(msg_in.clone(), &skill).unwrap();
    assert_eq!(
        serde_json::to_value(got).unwrap(),
        serde_json::to_value(msg_in).unwrap()
    );
}

#[test]
fn wasm_skill_dispatches_through_artifact_parts() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let host = WasmRedactorHost::new(dir).unwrap();
    let wasm = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/identity_redact_part.wasm"
    ));
    let skill = SkillId::new("fixture").expect("valid");
    host.register_skill_wasm(skill.clone(), wasm).unwrap();

    let art_in = Artifact {
        artifact_id: "a".into(),
        name: None,
        description: None,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("blob".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
    };
    let got = host.redact_artifact(art_in.clone(), &skill).unwrap();
    assert_eq!(
        serde_json::to_value(got).unwrap(),
        serde_json::to_value(art_in).unwrap()
    );
}

#[test]
fn preload_without_signing_pubkey_skips_sig_file() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill = "fixture";
    let manifest = br#"{"json_path":"$.x"}"#;
    let wasm = fs::read(fixture_path()).expect("read fixture");
    fs::write(temp.path().join(format!("{skill}.wasm")), &wasm).expect("write wasm");
    fs::write(temp.path().join(format!("{skill}.manifest.json")), manifest).expect("write manifest");

    let host = WasmRedactorHost::new(WasmBundlePath::new(temp.path())).expect("host");
    host.preload_skill_bundle(SkillId::new(skill).expect("valid"))
        .expect("preload");
}

#[test]
fn preload_with_signing_pubkey_requires_valid_sig() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill = "fixture";
    let manifest = br#"{"json_path":"$.x"}"#;
    let wasm = fs::read(fixture_path()).expect("read fixture");
    let signing_key = SigningKey::from_bytes(&[9u8; 32]);
    let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
    write_signed_bundle(temp.path(), skill, &signing_key, manifest, &wasm);

    let host = WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
    host.preload_skill_bundle(SkillId::new(skill).expect("valid"))
        .expect("preload");
}

#[test]
fn preload_with_signing_pubkey_rejects_missing_sig() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill = "fixture";
    let manifest = br#"{"json_path":"$.x"}"#;
    let wasm = fs::read(fixture_path()).expect("read fixture");
    fs::write(temp.path().join(format!("{skill}.wasm")), &wasm).expect("write wasm");
    fs::write(temp.path().join(format!("{skill}.manifest.json")), manifest).expect("write manifest");

    let signing_key = SigningKey::from_bytes(&[11u8; 32]);
    let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
    let host = WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
    let err = host
        .preload_skill_bundle(SkillId::new(skill).expect("valid"))
        .expect_err("missing sig");
    assert!(matches!(
        err,
        RedactionError::Signature(SignatureVerificationError::MissingSignatureFile { .. })
    ));
}

#[test]
fn preload_with_signing_pubkey_rejects_tampered_wasm() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill = "fixture";
    let manifest = br#"{"json_path":"$.x"}"#;
    let wasm = fs::read(fixture_path()).expect("read fixture");
    let signing_key = SigningKey::from_bytes(&[13u8; 32]);
    write_signed_bundle(temp.path(), skill, &signing_key, manifest, &wasm);

    let wasm_path = temp.path().join(format!("{skill}.wasm"));
    fs::write(&wasm_path, b"tampered").expect("tamper wasm");

    let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
    let host = WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
    let err = host
        .preload_skill_bundle(SkillId::new(skill).expect("valid"))
        .expect_err("tampered wasm");
    assert!(matches!(
        err,
        RedactionError::Signature(SignatureVerificationError::ManifestSha256Mismatch { .. })
            | RedactionError::Signature(SignatureVerificationError::WasmSha256Mismatch { .. })
            | RedactionError::Signature(SignatureVerificationError::SignatureVerificationFailed { .. })
    ));
}

#[test]
fn tier3_refusal_error_display_renders_reason_tag() {
    // Pure rendering check for the new error variant. The wrapper's
    // sentinel-detection behavior is covered by the existing wasm
    // dispatch tests against the identity fixture, which the real
    // Tier-3 skills' integration tests exercise once their fixtures
    // land.
    let err = RedactionError::Tier3Refusal(Some("UnauthorizedDataCategory".into()));
    assert_eq!(
        err.to_string(),
        "tier-3 skill refused redaction: UnauthorizedDataCategory"
    );
}

#[test]
fn register_skill_wasm_refused_when_signing_pubkey_configured() {
    let temp = tempfile::tempdir().expect("tempdir");
    let signing_key = SigningKey::from_bytes(&[19u8; 32]);
    let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
    let host = WasmRedactorHost::new_with_signing_pubkey(WasmBundlePath::new(temp.path()), Some(pubkey)).expect("host");
    let wasm = fs::read(fixture_path()).expect("read fixture");
    let err = host
        .register_skill_wasm(SkillId::new("anything").expect("valid"), &wasm)
        .expect_err("must refuse bypass when signing pubkey configured");
    assert!(matches!(err, RedactionError::Signature(_)));
}

#[test]
fn host_exposes_configured_bundles_base_and_signing_pubkey() {
    let dir = WasmBundlePath::new(std::env::temp_dir());
    let signing_key = SigningKey::from_bytes(&[9u8; 32]);
    let pubkey = Ed25519PublicKey::from_bytes(*signing_key.verifying_key().as_bytes());
    let host = WasmRedactorHost::new_with_signing_pubkey(dir.clone(), Some(pubkey)).unwrap();
    assert_eq!(host.bundles_base().as_path(), dir.as_path());
    assert_eq!(host.signing_pubkey().unwrap().as_bytes(), pubkey.as_bytes());
}

#[test]
fn redact_part_bytes_passthrough_without_registered_module() {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).unwrap();
    let payload = br#"{"x":1}"#;
    let out = host
        .redact_part_bytes(&SkillId::new("missing").expect("valid"), payload)
        .unwrap();
    assert_eq!(out, payload);
}

#[test]
fn register_skill_bundle_file_delegates_to_preload() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill = "fixture";
    let manifest = br#"{"json_path":"$.x"}"#;
    let wasm = fs::read(fixture_path()).expect("read fixture");
    fs::write(temp.path().join(format!("{skill}.wasm")), &wasm).expect("write wasm");
    fs::write(temp.path().join(format!("{skill}.manifest.json")), manifest).expect("write manifest");

    let host = WasmRedactorHost::new(WasmBundlePath::new(temp.path())).expect("host");
    host.register_skill_bundle_file(SkillId::new(skill).expect("valid"))
        .expect("register via alias");
    let out = host
        .redact_part_bytes(&SkillId::new(skill).expect("valid"), br#"{"x":1}"#)
        .unwrap();
    assert_eq!(out, br#"{"x":1}"#);
}

#[test]
fn register_skill_wasm_rejects_invalid_wasm_bytes() {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).unwrap();
    let err = host
        .register_skill_wasm(SkillId::new("bad").expect("valid"), b"not-wasm")
        .unwrap_err();
    assert!(matches!(err, RedactionError::WasmModule(_)));
}

#[test]
fn preload_skill_bundle_fails_when_manifest_missing() {
    let temp = tempfile::tempdir().expect("tempdir");
    let skill = "fixture";
    let wasm = fs::read(fixture_path()).expect("read fixture");
    fs::write(temp.path().join(format!("{skill}.wasm")), &wasm).expect("write wasm");
    // Intentionally omit the manifest file.

    let host = WasmRedactorHost::new(WasmBundlePath::new(temp.path())).expect("host");
    let err = host
        .preload_skill_bundle(SkillId::new(skill).expect("valid"))
        .expect_err("missing manifest must fail");
    assert!(matches!(err, RedactionError::WasmModule(_)));
}

#[test]
fn tier3_refusal_is_surfaced_from_redact_part_bytes() {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).unwrap();
    let wasm = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tier3_refuse.wasm"));
    let skill = SkillId::new("tier3").expect("valid");
    host.register_skill_wasm(skill.clone(), wasm).unwrap();

    let err = host.redact_part_bytes(&skill, br#"{"x":1}"#).unwrap_err();
    assert!(matches!(err, RedactionError::Tier3Refusal(Some(_))));
}

#[test]
fn tier3_refusal_is_surfaced_from_redact_message() {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).unwrap();
    let wasm = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tier3_refuse.wasm"));
    let skill = SkillId::new("tier3-msg").expect("valid");
    host.register_skill_wasm(skill.clone(), wasm).unwrap();

    let msg = Message {
        message_id: "m".into(),
        context_id: None,
        task_id: None,
        role: Role::Agent,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("x".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
        reference_task_ids: None,
    };
    let err = host.redact_message(msg, &skill).unwrap_err();
    assert!(matches!(err, RedactionError::Tier3Refusal(_)));
}

#[test]
fn tier3_refusal_is_surfaced_from_redact_artifact() {
    let host = WasmRedactorHost::new(WasmBundlePath::new(std::env::temp_dir())).unwrap();
    let wasm = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tier3_refuse.wasm"));
    let skill = SkillId::new("tier3-art").expect("valid");
    host.register_skill_wasm(skill.clone(), wasm).unwrap();

    let art = Artifact {
        artifact_id: "a".into(),
        name: None,
        description: None,
        parts: vec![a2a::types::Part {
            content: PartContent::Text("blob".into()),
            filename: None,
            media_type: None,
            metadata: None,
        }],
        metadata: None,
        extensions: None,
    };
    let err = host.redact_artifact(art, &skill).unwrap_err();
    assert!(matches!(err, RedactionError::Tier3Refusal(_)));
}
