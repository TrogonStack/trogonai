//! MCP policy bundle loader per ADR 0010 (`manifest.toml`, CEL/WASM members, NKey signature gate).

mod archive;
mod errors;
mod manifest;
mod manifest_toml;
mod signature;
mod validate;

pub use archive::{build_tar, extract_archive, resolve_manifest, BundleArchive};
pub use errors::BundleLoadError;
pub use manifest::{
    BundleManifest, BundleScope, Capabilities, ComponentEntry, ManifestDigest, ProgramEntry,
    SchemaEntry, Signing, GATEWAY_VERSION, HOST_TARGET_WIT, MANIFEST_DEPRECATED_FILENAME,
    MANIFEST_FILENAME, SIGNATURE_PATH,
};
pub use signature::{manifest_digest_bytes, parse_signature_bytes, TrustedKeys};
pub use validate::{
    hash_member, validate_members, LoadedComponent, LoadedProgram, LoadedSchema, ValidatedMembers,
};

use signature::{signature_path, verify_manifest_signature};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedBundle {
    pub manifest: BundleManifest,
    pub manifest_filename: String,
    pub manifest_bytes: Vec<u8>,
    pub manifest_digest: ManifestDigest,
    pub scope: BundleScope,
    pub signer_nkey: String,
    pub programs: Vec<LoadedProgram>,
    pub components: Vec<LoadedComponent>,
    pub schemas: Vec<LoadedSchema>,
}

pub fn load_bundle(
    archive_bytes: &[u8],
    trusted: &TrustedKeys,
) -> Result<LoadedBundle, BundleLoadError> {
    let archive = extract_archive(archive_bytes)?;
    verify_and_materialize(&archive, trusted)
}

pub fn verify_bundle(
    archive_bytes: &[u8],
    trusted: &TrustedKeys,
) -> Result<LoadedBundle, BundleLoadError> {
    load_bundle(archive_bytes, trusted)
}

pub fn load_bundle_from_archive(
    archive: &BundleArchive,
    trusted: &TrustedKeys,
) -> Result<LoadedBundle, BundleLoadError> {
    verify_and_materialize(archive, trusted)
}

fn verify_and_materialize(
    archive: &BundleArchive,
    trusted: &TrustedKeys,
) -> Result<LoadedBundle, BundleLoadError> {
    let (manifest_filename, manifest_bytes) = resolve_manifest(archive)?;

    let signature_bytes = archive
        .get(signature_path())
        .ok_or(BundleLoadError::SignatureMissing)?;

    let manifest = BundleManifest::parse(manifest_bytes)?;
    verify_manifest_signature(
        manifest_bytes,
        signature_bytes,
        &manifest.signing.nkey_pub,
        trusted,
    )?;

    manifest.validate()?;
    let scope = manifest.scope()?;
    let members = validate_members(&manifest, archive)?;
    let manifest_digest = ManifestDigest::from_bytes(manifest_bytes);

    Ok(LoadedBundle {
        signer_nkey: manifest.signing.nkey_pub.clone(),
        manifest,
        manifest_filename: manifest_filename.to_string(),
        manifest_bytes: manifest_bytes.to_vec(),
        manifest_digest,
        scope,
        programs: members.programs,
        components: members.components,
        schemas: members.schemas,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::manifest::HOST_TARGET_WIT;

    fn fixture_manifest_toml(program_hash: &str) -> String {
        format!(
            r#"
name = "acme/demo"
version = "1.0.0"
target_wit = "{HOST_TARGET_WIT}"
min_gateway_version = "0.0.1"
author = "platform"
created_at = "2026-05-28T00:00:00Z"
description = "demo"

[signing]
nkey_pub = "UABTRUSTED"

[[programs]]
id = "rule"
path = "policies/rule.cel"
sha256 = "{program_hash}"
class = "ingress_gate"
effect = "allow"
priority = 1
"#
        )
    }

    fn fixture_archive(program_body: &[u8]) -> Vec<u8> {
        let hash = hash_member(program_body);
        let manifest = fixture_manifest_toml(&hash);
        let mut archive = BundleArchive::default();
        archive.insert(MANIFEST_FILENAME, manifest.into_bytes());
        archive.insert("policies/rule.cel", program_body.to_vec());
        archive.insert(signature_path(), vec![0u8; 64]);
        build_tar(&archive)
    }

    #[test]
    fn load_bundle_rejects_missing_signature() {
        let hash = hash_member(b"true");
        let manifest = fixture_manifest_toml(&hash);
        let mut archive = BundleArchive::default();
        archive.insert(MANIFEST_FILENAME, manifest.into_bytes());
        archive.insert("policies/rule.cel", b"true".to_vec());
        let tar = build_tar(&archive);
        let trusted = TrustedKeys::from_allowlist(["UABTRUSTED"]);
        let error = load_bundle(&tar, &trusted).expect_err("missing sig");
        assert!(matches!(error, BundleLoadError::SignatureMissing));
    }

    #[test]
    fn load_bundle_reports_signature_dependency_gap_when_structure_valid() {
        let tar = fixture_archive(b"true");
        let trusted = TrustedKeys::from_allowlist(["UABTRUSTED"]);
        let error = load_bundle(&tar, &trusted).expect_err("nkeys unavailable");
        assert!(matches!(
            error,
            BundleLoadError::SignatureVerificationUnavailable { .. }
        ));
    }

    #[test]
    fn deprecated_manifest_filename_is_accepted() {
        let hash = hash_member(b"true");
        let manifest = fixture_manifest_toml(&hash);
        let mut archive = BundleArchive::default();
        archive.insert(MANIFEST_DEPRECATED_FILENAME, manifest.into_bytes());
        archive.insert("policies/rule.cel", b"true".to_vec());
        archive.insert(signature_path(), vec![0u8; 64]);
        let tar = build_tar(&archive);
        let trusted = TrustedKeys::from_allowlist(["UABTRUSTED"]);
        let error = load_bundle(&tar, &trusted).expect_err("nkeys unavailable");
        assert!(matches!(
            error,
            BundleLoadError::SignatureVerificationUnavailable { .. }
        ));
    }
}
