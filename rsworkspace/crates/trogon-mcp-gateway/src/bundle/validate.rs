use sha2::{Digest, Sha256};

use super::archive::BundleArchive;
use super::errors::BundleLoadError;
use super::manifest::{
    normalize_member_path, BundleManifest, ComponentEntry, ProgramEntry, SchemaEntry,
};

const IGNORED_PATH_PREFIXES: &[&str] = &["README.md", "signatures/"];

pub fn validate_members(
    manifest: &BundleManifest,
    archive: &BundleArchive,
) -> Result<ValidatedMembers, BundleLoadError> {
    let declared = manifest.declared_member_paths();
    let mut programs = Vec::new();
    let mut components = Vec::new();
    let mut schemas = Vec::new();

    for program in &manifest.programs {
        let bytes = load_member(archive, &program.path)?;
        verify_hash(&program.path, &program.sha256, &bytes)?;
        enforce_size_limit(&program.path, bytes.len())?;
        programs.push(LoadedProgram {
            entry: program.clone(),
            bytes,
        });
    }

    for component in &manifest.components {
        let bytes = load_member(archive, &component.path)?;
        verify_hash(&component.path, &component.sha256, &bytes)?;
        enforce_size_limit(&component.path, bytes.len())?;
        components.push(LoadedComponent {
            entry: component.clone(),
            bytes,
        });
    }

    for schema in &manifest.schemas {
        let bytes = load_member(archive, &schema.path)?;
        verify_hash(&schema.path, &schema.sha256, &bytes)?;
        enforce_size_limit(&schema.path, bytes.len())?;
        schemas.push(LoadedSchema {
            entry: schema.clone(),
            bytes,
        });
    }

    reject_unknown_members(archive, &declared)?;

    Ok(ValidatedMembers {
        programs,
        components,
        schemas,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedMembers {
    pub programs: Vec<LoadedProgram>,
    pub components: Vec<LoadedComponent>,
    pub schemas: Vec<LoadedSchema>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedProgram {
    pub entry: ProgramEntry,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedComponent {
    pub entry: ComponentEntry,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedSchema {
    pub entry: SchemaEntry,
    pub bytes: Vec<u8>,
}

fn load_member(archive: &BundleArchive, path: &str) -> Result<Vec<u8>, BundleLoadError> {
    let normalized = normalize_member_path(path);
    let bytes = archive
        .get(&normalized)
        .ok_or_else(|| BundleLoadError::MemberMissing {
            path: normalized.clone(),
        })?
        .to_vec();
    enforce_size_limit(&normalized, bytes.len())?;
    Ok(bytes)
}

fn enforce_size_limit(path: &str, size: usize) -> Result<(), BundleLoadError> {
    let limit = BundleManifest::member_size_limit(path);
    if size > limit {
        return Err(BundleLoadError::MemberTooLarge {
            path: normalize_member_path(path),
            size,
            limit,
        });
    }
    Ok(())
}

pub fn hash_member(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn verify_hash(path: &str, declared: &str, bytes: &[u8]) -> Result<(), BundleLoadError> {
    let actual = hash_member(bytes);
    let expected = declared.strip_prefix("sha256:").unwrap_or(declared).to_ascii_lowercase();
    if actual != expected {
        return Err(BundleLoadError::ContentHashMismatch {
            path: normalize_member_path(path),
            expected: expected.clone(),
            actual,
        });
    }
    Ok(())
}

fn reject_unknown_members(
    archive: &BundleArchive,
    declared: &[String],
) -> Result<(), BundleLoadError> {
    let declared_set: std::collections::BTreeSet<_> = declared.iter().cloned().collect();
    for path in archive.paths() {
        if is_ignored_path(path) {
            continue;
        }
        if path == "manifest.toml" || path == "bundle.toml" {
            continue;
        }
        if !declared_set.contains(path) {
            return Err(BundleLoadError::UnknownMember { path: path.clone() });
        }
    }
    Ok(())
}

fn is_ignored_path(path: &str) -> bool {
    IGNORED_PATH_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::archive::BundleArchive;
    use crate::bundle::manifest::{
        BundleManifest, Capabilities, ProgramEntry, Signing, HOST_TARGET_WIT,
    };

    fn sample_manifest(program_hash: &str) -> BundleManifest {
        BundleManifest {
            name: "acme/demo".into(),
            version: "1.0.0".into(),
            target_wit: HOST_TARGET_WIT.into(),
            min_gateway_version: "0.0.1".into(),
            cel_version: None,
            author: "platform".into(),
            created_at: "2026-05-28T00:00:00Z".into(),
            description: "demo".into(),
            capabilities: Capabilities::default(),
            signing: Signing {
                nkey_pub: "UABTRUSTED".into(),
            },
            programs: vec![ProgramEntry {
                id: "rule".into(),
                path: "policies/rule.cel".into(),
                sha256: program_hash.into(),
                class: "ingress_gate".into(),
                effect: "allow".into(),
                priority: 1,
            }],
            components: vec![],
            schemas: vec![],
        }
    }

    #[test]
    fn content_hash_mismatch_rejected() {
        let body = b"true";
        let manifest = sample_manifest("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let mut archive = BundleArchive::default();
        archive.insert("policies/rule.cel", body.to_vec());
        let error = validate_members(&manifest, &archive).expect_err("hash mismatch");
        assert!(matches!(
            error,
            BundleLoadError::ContentHashMismatch { .. }
        ));
    }

    #[test]
    fn unknown_member_rejected() {
        let body = b"true";
        let hash = hash_member(body);
        let manifest = sample_manifest(&hash);
        let mut archive = BundleArchive::default();
        archive.insert("policies/rule.cel", body.to_vec());
        archive.insert("policies/extra.cel", b"false".to_vec());
        let error = validate_members(&manifest, &archive).expect_err("unknown member");
        assert!(matches!(error, BundleLoadError::UnknownMember { .. }));
    }

    #[test]
    fn matching_hash_accepted() {
        let body = b"true";
        let hash = hash_member(body);
        let manifest = sample_manifest(&hash);
        let mut archive = BundleArchive::default();
        archive.insert("policies/rule.cel", body.to_vec());
        validate_members(&manifest, &archive).expect("valid");
    }
}
