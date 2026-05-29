use std::fs;
use std::path::Path;

use serde::Serialize;
use trogon_mcp_gateway::bundle::{
    build_tar, load_bundle, BundleArchive, BundleLoadError, LoadedBundle, TrustedKeys,
    MANIFEST_FILENAME,
};

use crate::output::emit_json;

#[derive(Debug, Serialize)]
pub struct BundleValidateResult {
    pub result: &'static str,
    pub manifest: BundleManifestSummary,
    pub signature: SignatureStatus,
    pub programs: Vec<ProgramSummary>,
}

#[derive(Debug, Serialize)]
pub struct BundleManifestSummary {
    pub name: String,
    pub version: String,
    pub manifest_filename: String,
    pub manifest_digest: String,
    pub signer_nkey: String,
}

#[derive(Debug, Serialize)]
pub struct SignatureStatus {
    pub verified: bool,
    pub signer_nkey: String,
}

#[derive(Debug, Serialize)]
pub struct ProgramSummary {
    pub id: String,
    pub path: String,
    pub class: String,
    pub effect: String,
}

#[derive(Debug, Serialize)]
pub struct BundleValidateError {
    pub result: &'static str,
    pub error: String,
}

pub fn run(path: &Path, trusted_keys_path: &Path, pretty: bool) -> Result<(), BundleOutcome> {
    let trusted = load_trusted_keys(trusted_keys_path)?;
    let archive_bytes = read_bundle_bytes(path)?;
    match load_bundle(&archive_bytes, &trusted) {
        Ok(bundle) => {
            let payload = summarize_valid(bundle);
            emit_json(&payload, pretty).map_err(|error| BundleOutcome::Runtime(error.to_string()))?;
            Ok(())
        }
        Err(error) => Err(BundleOutcome::Validation(error)),
    }
}

#[derive(Debug)]
pub enum BundleOutcome {
    Validation(BundleLoadError),
    Runtime(String),
}

impl BundleOutcome {
    pub fn message(&self) -> String {
        match self {
            Self::Validation(error) => error.to_string(),
            Self::Runtime(message) => message.clone(),
        }
    }

    pub fn emit_error_json(&self, pretty: bool) -> Result<(), String> {
        let payload = BundleValidateError {
            result: "INVALID",
            error: self.message(),
        };
        emit_json(&payload, pretty).map_err(|error| error.to_string())
    }
}

fn summarize_valid(bundle: LoadedBundle) -> BundleValidateResult {
    BundleValidateResult {
        result: "VALID",
        signature: SignatureStatus {
            verified: true,
            signer_nkey: bundle.signer_nkey.clone(),
        },
        manifest: BundleManifestSummary {
            name: bundle.manifest.name.clone(),
            version: bundle.manifest.version.clone(),
            manifest_filename: bundle.manifest_filename.clone(),
            manifest_digest: bundle.manifest_digest.as_hex().to_string(),
            signer_nkey: bundle.signer_nkey.clone(),
        },
        programs: bundle
            .programs
            .iter()
            .map(|program| ProgramSummary {
                id: program.entry.id.clone(),
                path: program.entry.path.clone(),
                class: program.entry.class.clone(),
                effect: program.entry.effect.clone(),
            })
            .collect(),
    }
}

fn load_trusted_keys(path: &Path) -> Result<TrustedKeys, BundleOutcome> {
    let raw = fs::read_to_string(path)
        .map_err(|error| BundleOutcome::Runtime(format!("read trusted keys {}: {error}", path.display())))?;
    let keys: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect();
    if keys.is_empty() {
        return Err(BundleOutcome::Runtime(format!(
            "trusted keys file {} contains no keys",
            path.display()
        )));
    }
    Ok(TrustedKeys::from_allowlist(keys))
}

fn read_bundle_bytes(path: &Path) -> Result<Vec<u8>, BundleOutcome> {
    if path.is_file() {
        return fs::read(path)
            .map_err(|error| BundleOutcome::Runtime(format!("read bundle {}: {error}", path.display())));
    }
    if path.is_dir() {
        return tar_from_directory(path);
    }
    Err(BundleOutcome::Runtime(format!(
        "bundle path {} is not a file or directory",
        path.display()
    )))
}

fn tar_from_directory(path: &Path) -> Result<Vec<u8>, BundleOutcome> {
    let mut archive = BundleArchive::default();
    for entry in fs::read_dir(path)
        .map_err(|error| BundleOutcome::Runtime(format!("read bundle dir {}: {error}", path.display())))?
    {
        let entry = entry.map_err(|error| BundleOutcome::Runtime(error.to_string()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| BundleOutcome::Runtime(error.to_string()))?;
        if !file_type.is_file() {
            continue;
        }
        let entry_path = entry.path();
        let relative = entry_path
            .strip_prefix(path)
            .map_err(|error| BundleOutcome::Runtime(error.to_string()))?;
        let member_path = relative
            .to_str()
            .ok_or_else(|| BundleOutcome::Runtime("bundle member path is not UTF-8".into()))?;
        let bytes = fs::read(&entry_path)
            .map_err(|error| BundleOutcome::Runtime(format!("read bundle member {member_path}: {error}")))?;
        archive.insert(member_path, bytes);
    }
    if !archive.files.contains_key(MANIFEST_FILENAME) {
        return Err(BundleOutcome::Runtime(format!(
            "bundle directory {} is missing {MANIFEST_FILENAME}",
            path.display()
        )));
    }
    Ok(build_tar(&archive))
}

#[cfg(test)]
pub mod fixtures {
    use nkeys::KeyPair;
    use trogon_mcp_gateway::bundle::{
        build_tar, hash_member, manifest_digest_bytes, signature_path, BundleArchive, HOST_TARGET_WIT,
        MANIFEST_FILENAME,
    };

    pub fn signed_tar(kp: &KeyPair, cel_body: &[u8]) -> Vec<u8> {
        let hash = hash_member(cel_body);
        let manifest = format!(
            r#"
name = "acme/demo"
version = "1.0.0"
target_wit = "{HOST_TARGET_WIT}"
min_gateway_version = "0.0.1"
author = "platform"
created_at = "2026-05-28T00:00:00Z"
description = "demo"

[signing]
nkey_pub = "{}"

[[programs]]
id = "rule"
path = "policies/rule.cel"
sha256 = "{hash}"
class = "ingress_gate"
effect = "allow"
priority = 1
"#,
            kp.public_key()
        );
        let manifest_bytes = manifest.into_bytes();
        let sig = kp.sign(&manifest_digest_bytes(&manifest_bytes)).expect("sign");
        let mut archive = BundleArchive::default();
        archive.insert(MANIFEST_FILENAME, manifest_bytes);
        archive.insert("policies/rule.cel", cel_body.to_vec());
        archive.insert(signature_path(), sig);
        build_tar(&archive)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::fixtures::signed_tar;
    use super::*;
    use nkeys::KeyPair;
    use tempfile::NamedTempFile;

    #[test]
    fn validate_accepts_signed_fixture() {
        let kp = KeyPair::new_user();
        let tar = signed_tar(&kp, b"true");
        let mut bundle_file = NamedTempFile::new().expect("temp bundle");
        bundle_file.write_all(&tar).expect("write bundle");
        let mut keys_file = NamedTempFile::new().expect("temp keys");
        writeln!(keys_file, "{}", kp.public_key()).expect("write key");

        run(bundle_file.path(), keys_file.path(), false).expect("valid bundle");
    }

    #[test]
    fn validate_rejects_untrusted_signer() {
        let kp = KeyPair::new_user();
        let tar = signed_tar(&kp, b"true");
        let mut bundle_file = NamedTempFile::new().expect("temp bundle");
        bundle_file.write_all(&tar).expect("write bundle");
        let other = KeyPair::new_user();
        let mut keys_file = NamedTempFile::new().expect("temp keys");
        writeln!(keys_file, "{}", other.public_key()).expect("write key");

        let error = run(bundle_file.path(), keys_file.path(), false).expect_err("invalid");
        assert!(matches!(error, BundleOutcome::Validation(_)));
    }
}
